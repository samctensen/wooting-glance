use crate::{
    device::Device,
    layout::{COUNT, Indicator, Service},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::{self, ErrorKind},
    os::unix::net::UnixDatagram,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use wooting_glance::{
    protocol,
    recovery::{self, Keyboard, Recovery},
};

const SOCKET: &str = "agents.sock";

#[derive(Deserialize)]
struct Hook {
    session_id: Option<String>,
    hook_event_name: String,
    agent_id: Option<String>,
    #[serde(rename = "hookEventName")]
    grok_event_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
enum Status {
    Idle,
    Working,
    Approval,
}

#[derive(Serialize, Deserialize)]
struct Event {
    service: u8,
    session: String,
    actor: Option<String>,
    status: Status,
    clear_session: bool,
}

// Deserialize only lifecycle metadata. Prompt and tool payloads stay out of IPC.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrokHook {
    session_id: String,
    hook_event_name: String,
    subagent_id: Option<String>,
    notification_type: Option<String>,
}

fn read_event(input: impl io::Read, service: Service) -> Result<Option<Event>> {
    Ok(match service {
        Service::Grok => serde_json::from_reader::<_, GrokHook>(input)?.event(),
        Service::Claude | Service::Codex => {
            serde_json::from_reader::<_, Hook>(input)?.event(service as u8)
        }
    })
}

impl GrokHook {
    fn event(self) -> Option<Event> {
        let (status, session) = match self.hook_event_name.as_str() {
            "subagent_start" => (Status::Working, self.subagent_id?),
            "subagent_stop" => (Status::Idle, self.subagent_id?),
            "user_prompt_submit"
            | "post_tool_use"
            | "post_tool_use_failure"
            | "permission_denied" => (Status::Working, self.session_id),
            "notification" if self.notification_type.as_deref() == Some("permission_prompt") => {
                (Status::Approval, self.session_id)
            }
            "stop" | "stop_cancelled" | "stop_failure" | "session_end" => {
                (Status::Idle, self.session_id)
            }
            _ => return None,
        };
        if session.is_empty() || session.len() > 128 {
            return None;
        }
        Some(Event {
            service: Service::Grok as u8,
            session,
            actor: None,
            status,
            clear_session: false,
        })
    }
}

pub fn forward(dir: &Path, service: Service) -> Result<()> {
    let socket = UnixDatagram::unbound()?;
    if socket.connect(dir.join(SOCKET)).is_err() {
        return Ok(());
    }
    let Some(event) = read_event(io::stdin().lock(), service)? else {
        return Ok(());
    };
    socket.set_nonblocking(true)?;
    let _ = socket.send(&serde_json::to_vec(&event)?);
    Ok(())
}

impl Hook {
    fn event(self, service: u8) -> Option<Event> {
        // Grok imports Claude's hooks and also supplies the PascalCase Claude event name.
        if self.grok_event_name.is_some() {
            return None;
        }
        let session = self.session_id?;
        if session.is_empty()
            || session.len() > 128
            || self
                .agent_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return None;
        }
        let name = self.hook_event_name.as_str();
        if matches!(name, "SubagentStart" | "SubagentStop") && self.agent_id.is_none() {
            return None;
        }
        let status = match name {
            "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "SubagentStart" => {
                Status::Working
            }
            "PermissionRequest" => Status::Approval,
            "Stop" | "SubagentStop" | "SessionEnd" | "Interrupt" | "StopFailure" => Status::Idle,
            _ => return None,
        };
        Some(Event {
            service,
            session,
            clear_session: self.agent_id.is_none() && matches!(name, "SessionEnd" | "Interrupt"),
            actor: self.agent_id,
            status,
        })
    }
}

type Actor = (u8, String, Option<String>);

#[derive(Default)]
struct Activity(HashMap<Actor, (Status, Instant)>);

impl Activity {
    fn update(&mut self, event: Event, now: Instant) {
        if event.clear_session {
            self.0.retain(|(service, session, _), _| {
                *service != event.service || session != &event.session
            });
            return;
        }
        let key = (event.service, event.session, event.actor);
        if event.status == Status::Idle {
            self.0.remove(&key);
        } else {
            self.0.insert(key, (event.status, now));
        }
    }
    fn states(&mut self, now: Instant) -> [Status; COUNT] {
        self.0
            .retain(|_, (_, last)| now.duration_since(*last) < Duration::from_secs(3600));
        let mut result = [Status::Idle; COUNT];
        for ((service, _, _), (status, _)) in &self.0 {
            result[*service as usize] = result[*service as usize].max(*status);
        }
        result
    }
    fn poll(&mut self, socket: &UnixDatagram) -> Result<()> {
        let mut buf = [0; 1024];
        match socket.recv(&mut buf) {
            Ok(n) => {
                if let Ok(event) = serde_json::from_slice::<Event>(&buf[..n])
                    && (event.service as usize) < COUNT
                    && event.session.len() <= 128
                {
                    self.update(event, Instant::now());
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}

// Full intensity is the brand RGB value. Scale all three channels together.
const PULSE: [u16; 8] = [255, 223, 159, 96, 64, 96, 159, 223];

fn frame(layout: &[Indicator], states: [Status; COUNT], elapsed: Duration) -> Vec<(u8, [u8; 3])> {
    let phase = (elapsed.as_millis() / 300 % PULSE.len() as u128) as usize;
    layout
        .iter()
        .map(|entry| {
            let level = match states[entry.agent as usize] {
                Status::Idle => 0,
                Status::Working => 255,
                Status::Approval => PULSE[phase],
            };
            (
                entry.led,
                entry
                    .color
                    .map(|c| ((u16::from(c) * level + 127) / 255) as u8),
            )
        })
        .collect()
}

fn paint(
    device: &Device,
    slot: u8,
    baseline: &[u8],
    leds: &[(u8, [u8; 3])],
    keep: &mut bool,
) -> Result<()> {
    if *keep && let Ok(body) = protocol::bar_frame(baseline, leds, true) {
        device.write_profile(slot, &body)?;
        if protocol::verify_colors(&device.colors()?, leds).is_ok() {
            return Ok(());
        }
    }
    if *keep {
        eprintln!(
            "Saved effects did not accept the configured indicators. Using a dedicated bar until Glance exits."
        );
    }
    *keep = false;
    device.write_profile(slot, &protocol::bar_frame(baseline, leds, false)?)?;
    protocol::verify_colors(&device.colors()?, leds)
}

pub fn watch(dir: &Path, layout: &[Indicator]) -> Result<()> {
    let recovery = Recovery::acquire(dir)?;
    let path = dir.join(SOCKET);
    // This process holds the single Glance lock, so any socket here is stale.
    if path.exists() {
        fs::remove_file(&path)?;
    }
    let socket = UnixDatagram::bind(&path)?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed))?;
    let device = Device::open()?;
    if dir.join("recovery.json").exists() {
        recovery::restore(&device, &device.identity, &recovery)?;
    }
    let mut activity = Activity::default();
    for entry in layout {
        eprintln!("Watching {:?} on LED {}.", entry.agent, entry.led + 1);
    }
    let start = Instant::now();
    let dark = frame(layout, [Status::Idle; COUNT], Duration::ZERO);
    recovery::with_overlay(
        &device,
        &device.identity,
        &recovery,
        &dark,
        |slot, baseline| {
            let mut keep = true;
            paint(&device, slot, baseline, &dark, &mut keep)?;
            let mut last = dark.clone();
            let mut last_profile_check = Instant::now();
            while running.load(Ordering::Relaxed) {
                activity.poll(&socket)?;
                let now = Instant::now();
                let leds = frame(layout, activity.states(now), now.duration_since(start));
                if leds != last || last_profile_check.elapsed() >= Duration::from_secs(2) {
                    ensure!(
                        device.active_slot()? == slot && device.stored_profile(slot)? == baseline,
                        "keyboard profile changed during watch; restart Glance"
                    );
                    last_profile_check = now;
                }
                if leds != last {
                    paint(&device, slot, baseline, &leds, &mut keep)?;
                    last = leds;
                }
            }
            Ok(())
        },
    )?;
    fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn send(a: &mut Activity, service: u8, session: &str, actor: Option<&str>, name: &str) {
        let hook = Hook {
            session_id: Some(session.into()),
            grok_event_name: None,
            agent_id: actor.map(str::to_owned),
            hook_event_name: name.into(),
        };
        if let Some(e) = hook.event(service) {
            a.update(e, Instant::now());
        }
    }
    #[test]
    fn parent_stop_waits_for_children_and_approval_outranks_other_work() {
        let mut a = Activity::default();
        for actor in [Some("child"), Some("sibling")] {
            send(&mut a, 0, "a", actor, "SubagentStart");
        }
        send(&mut a, 0, "a", Some("child"), "PermissionRequest");
        send(&mut a, 0, "a", Some("sibling"), "PostToolUse");
        send(&mut a, 0, "a", None, "Stop");
        send(&mut a, 1, "b", None, "UserPromptSubmit");
        assert_eq!(
            a.states(Instant::now()),
            [Status::Approval, Status::Working, Status::Idle]
        );
        send(&mut a, 0, "a", Some("child"), "PostToolUse");
        send(&mut a, 0, "a", Some("sibling"), "SubagentStop");
        assert_eq!(
            a.states(Instant::now()),
            [Status::Working, Status::Working, Status::Idle]
        );
        send(&mut a, 0, "a", Some("child"), "SubagentStop");
        assert_eq!(
            a.states(Instant::now()),
            [Status::Idle, Status::Working, Status::Idle]
        );
    }
    #[test]
    fn cancellation_only_clears_the_matching_task() {
        let mut a = Activity::default();
        send(&mut a, 1, "x", Some("a"), "PermissionRequest");
        send(&mut a, 1, "y", None, "UserPromptSubmit");
        send(&mut a, 1, "x", None, "Interrupt");
        assert_eq!(
            a.states(Instant::now()),
            [Status::Idle, Status::Working, Status::Idle]
        );
        send(&mut a, 1, "y", None, "Stop");
        assert_eq!(a.states(Instant::now()), [Status::Idle; COUNT]);
    }
    #[test]
    fn grok_routes_native_events_once_and_tracks_parallel_sessions() {
        let mut activity = Activity::default();
        let now = Instant::now();
        let mut check = |json: &str, expected| {
            if let Some(event) = read_event(json.as_bytes(), Service::Grok).unwrap() {
                activity.update(event, now);
            }
            assert!(
                read_event(json.as_bytes(), Service::Claude)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(activity.states(now), [Status::Idle, Status::Idle, expected]);
        };
        check(
            r#"{"sessionId":"main","hookEventName":"user_prompt_submit","hook_event_name":"UserPromptSubmit"}"#,
            Status::Working,
        );
        check(
            r#"{"sessionId":"main","hookEventName":"subagent_start","hook_event_name":"SubagentStart","subagentId":"child"}"#,
            Status::Working,
        );
        check(
            r#"{"sessionId":"child","hookEventName":"notification","hook_event_name":"Notification","notificationType":"permission_prompt"}"#,
            Status::Approval,
        );
        check(
            r#"{"sessionId":"main","hookEventName":"stop","hook_event_name":"Stop"}"#,
            Status::Approval,
        );
        check(
            r#"{"sessionId":"child","hookEventName":"post_tool_use","hook_event_name":"PostToolUse"}"#,
            Status::Working,
        );
        check(
            r#"{"sessionId":"child","hookEventName":"subagent_stop","hook_event_name":"SubagentStop","subagentId":"child"}"#,
            Status::Idle,
        );
        check(
            r#"{"sessionId":"main","hookEventName":"notification","hook_event_name":"Notification","notificationType":"idle_prompt"}"#,
            Status::Idle,
        );
        check(
            r#"{"sessionId":"main","hookEventName":"user_prompt_submit","hook_event_name":"UserPromptSubmit"}"#,
            Status::Working,
        );
        check(
            r#"{"sessionId":"main","hookEventName":"stop_cancelled","hook_event_name":"StopCancelled"}"#,
            Status::Idle,
        );
    }

    #[test]
    fn json_layout_paints_only_assigned_positions() {
        let layout = crate::layout::parse(r##"{"leds":{"7":{"agent":"grok","color":"#AA2200"},"2":{"agent":"grok","color":"#223344"},"10":{"agent":"codex","color":"#10A37F"}}}"##).unwrap();
        let states = [Status::Idle, Status::Idle, Status::Working];
        assert_eq!(
            frame(&layout, states, Duration::ZERO),
            [(1, [34, 51, 68]), (6, [170, 34, 0]), (9, [0; 3])]
        );
    }

    #[test]
    fn only_the_waiting_service_changes_intensity() {
        let layout = crate::layout::parse(crate::layout::DEFAULT).unwrap();
        let states = [Status::Approval, Status::Working, Status::Idle];
        let full = frame(&layout, states, Duration::ZERO);
        let dim = frame(&layout, states, Duration::from_millis(1200));
        assert_eq!(full, [(0, [217, 119, 87]), (9, [16, 163, 127])]);
        assert_eq!(dim[0], (0, [54, 30, 22]));
        assert_eq!(dim[1], full[1]);
        assert_eq!(
            frame(&layout, [Status::Idle; COUNT], Duration::ZERO),
            [(0, [0; 3]), (9, [0; 3])]
        );
    }
}
