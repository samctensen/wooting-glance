use crate::device::Device;
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
    session_id: String,
    hook_event_name: String,
    agent_id: Option<String>,
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
pub fn forward(dir: &Path, service: &str) -> Result<()> {
    let socket = UnixDatagram::unbound()?;
    if socket.connect(dir.join(SOCKET)).is_err() {
        return Ok(());
    }
    let hook: Hook = serde_json::from_reader(io::stdin().lock())?;
    let Some(event) = hook.event(if service == "claude" { 0 } else { 1 }) else {
        return Ok(());
    };
    socket.set_nonblocking(true)?;
    let _ = socket.send(&serde_json::to_vec(&event)?);
    Ok(())
}

impl Hook {
    fn event(self, service: u8) -> Option<Event> {
        if self.session_id.is_empty()
            || self.session_id.len() > 128
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
            session: self.session_id,
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
    fn states(&mut self, now: Instant) -> [Status; 2] {
        self.0
            .retain(|_, (_, last)| now.duration_since(*last) < Duration::from_secs(3600));
        let mut result = [Status::Idle; 2];
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
                    && event.service <= 1
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
type Layout = [(u8, [u8; 3]); 2];
const PULSE: [u16; 8] = [255, 223, 159, 96, 64, 96, 159, 223];

fn frame(layout: Layout, states: [Status; 2], elapsed: Duration) -> [(u8, [u8; 3]); 2] {
    let phase = (elapsed.as_millis() / 300 % PULSE.len() as u128) as usize;
    std::array::from_fn(|i| {
        let level = match states[i] {
            Status::Idle => 0,
            Status::Working => 255,
            Status::Approval => PULSE[phase],
        };
        (
            layout[i].0,
            layout[i]
                .1
                .map(|c| ((u16::from(c) * level + 127) / 255) as u8),
        )
    })
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
            "Saved effects did not accept both indicators. Using a dedicated bar until Glance exits."
        );
    }
    *keep = false;
    device.write_profile(slot, &protocol::preview(baseline, leds)?)?;
    protocol::verify_colors(&device.colors()?, leds)
}

pub fn watch(dir: &Path, layout: Layout) -> Result<()> {
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
    eprintln!(
        "Watching Claude on LED {} and Codex on LED {}.",
        layout[0].0 + 1,
        layout[1].0 + 1
    );
    let start = Instant::now();
    let dark = frame(layout, [Status::Idle; 2], Duration::ZERO);
    recovery::with_overlay(
        &device,
        &device.identity,
        &recovery,
        &dark,
        |slot, baseline| {
            let mut keep = true;
            paint(&device, slot, baseline, &dark, &mut keep)?;
            let mut last = dark;
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
            session_id: session.into(),
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
            [Status::Approval, Status::Working]
        );
        send(&mut a, 0, "a", Some("child"), "PostToolUse");
        send(&mut a, 0, "a", Some("sibling"), "SubagentStop");
        assert_eq!(a.states(Instant::now()), [Status::Working, Status::Working]);
        send(&mut a, 0, "a", Some("child"), "SubagentStop");
        assert_eq!(a.states(Instant::now()), [Status::Idle, Status::Working]);
    }
    #[test]
    fn cancellation_only_clears_the_matching_task() {
        let mut a = Activity::default();
        send(&mut a, 1, "x", Some("a"), "PermissionRequest");
        send(&mut a, 1, "y", None, "UserPromptSubmit");
        send(&mut a, 1, "x", None, "Interrupt");
        assert_eq!(a.states(Instant::now()), [Status::Idle, Status::Working]);
        send(&mut a, 1, "y", None, "Stop");
        assert_eq!(a.states(Instant::now()), [Status::Idle; 2]);
    }
    #[test]
    fn only_the_waiting_service_changes_intensity() {
        let layout = [(0, [217, 119, 87]), (9, [16, 163, 127])];
        let states = [Status::Approval, Status::Working];
        let full = frame(layout, states, Duration::ZERO);
        let dim = frame(layout, states, Duration::from_millis(1200));
        assert_eq!(full, layout);
        assert_eq!(dim[0], (0, [54, 30, 22]));
        assert_eq!(dim[1], full[1]);
        assert_eq!(
            frame(layout, [Status::Idle; 2], Duration::ZERO),
            [(0, [0; 3]), (9, [0; 3])]
        );
    }
}
