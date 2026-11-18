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
    Ready,
    Working,
    Approval,
}

#[derive(Serialize, Deserialize)]
struct Event {
    service: u8,
    session: String,
    status: Status,
}

// Deserialize only lifecycle metadata. Prompt and tool payloads stay out of IPC.
pub fn forward(dir: &Path, service: &str) -> Result<()> {
    let socket = UnixDatagram::unbound()?;
    if socket.connect(dir.join(SOCKET)).is_err() {
        return Ok(());
    }
    let hook: Hook = serde_json::from_reader(io::stdin().lock())?;
    if hook.agent_id.is_some() || hook.session_id.is_empty() || hook.session_id.len() > 128 {
        return Ok(());
    }
    let status = match hook.hook_event_name.as_str() {
        "UserPromptSubmit" | "PostToolUse" => Status::Working,
        "PermissionRequest" => Status::Approval,
        "Stop" => Status::Ready,
        "SessionEnd" | "Interrupt" | "StopFailure" => Status::Idle,
        _ => return Ok(()),
    };
    let event = Event {
        service: if service == "claude" { 0 } else { 1 },
        session: hook.session_id,
        status,
    };
    socket.set_nonblocking(true)?;
    let _ = socket.send(&serde_json::to_vec(&event)?);
    Ok(())
}

#[derive(Default)]
struct Activity(HashMap<(u8, String), (Status, Instant)>);

impl Activity {
    fn update(&mut self, event: Event, now: Instant) {
        let key = (event.service, event.session);
        if event.status == Status::Idle {
            self.0.remove(&key);
        } else {
            self.0.insert(key, (event.status, now));
        }
    }
    fn states(&mut self, now: Instant) -> [Status; 2] {
        self.0.retain(|_, (status, start)| {
            now.duration_since(*start)
                < if *status == Status::Ready {
                    Duration::from_secs(8)
                } else {
                    Duration::from_secs(3600)
                }
        });
        let mut states = [Status::Idle; 2];
        for ((service, _), (status, _)) in &self.0 {
            states[*service as usize] = states[*service as usize].max(*status);
        }
        states
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

fn frame(states: [Status; 2]) -> [(u8, [u8; 3]); 2] {
    let colors = states.map(|state| match state {
        Status::Idle => [0, 0, 0],
        Status::Working => [0, 100, 255],
        Status::Approval => [255, 128, 0],
        Status::Ready => [0, 255, 64],
    });
    [(0, colors[0]), (9, colors[1])]
}

pub fn watch(dir: &Path) -> Result<()> {
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
    eprintln!("Watching Claude on LED 1 and Codex on LED 10.");
    while running.load(Ordering::Relaxed) {
        activity.poll(&socket)?;
        let mut shown = activity.states(Instant::now());
        if shown == [Status::Idle; 2] {
            continue;
        }
        recovery::with_overlay(
            &device,
            &device.identity,
            &recovery,
            &frame(shown),
            |slot, baseline| {
                protocol::verify_colors(&device.colors()?, &frame(shown))?;
                eprintln!("Claude {:?} · Codex {:?}", shown[0], shown[1]);
                while running.load(Ordering::Relaxed) {
                    activity.poll(&socket)?;
                    let next = activity.states(Instant::now());
                    if next == [Status::Idle; 2] {
                        break;
                    }
                    if next == shown {
                        continue;
                    }
                    ensure!(
                        device.active_slot()? == slot && device.stored_profile(slot)? == baseline,
                        "keyboard profile changed during watch; restart Glance"
                    );
                    let leds = frame(next);
                    device.write_profile(slot, &protocol::preview(baseline, &leds)?)?;
                    protocol::verify_colors(&device.colors()?, &leds)?;
                    shown = next;
                    eprintln!("Claude {:?} · Codex {:?}", shown[0], shown[1]);
                }
                Ok(())
            },
        )?;
    }
    fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_sessions_and_ready_expiration() {
        let mut a = Activity::default();
        let now = Instant::now();
        for (service, id, status) in [
            (0, "a", Status::Approval),
            (0, "b", Status::Working),
            (1, "a", Status::Ready),
        ] {
            a.update(
                Event {
                    service,
                    session: id.into(),
                    status,
                },
                now,
            );
        }
        assert_eq!(a.states(now), [Status::Approval, Status::Ready]);
        a.update(
            Event {
                service: 0,
                session: "a".into(),
                status: Status::Idle,
            },
            now,
        );
        assert_eq!(
            a.states(now + Duration::from_secs(9)),
            [Status::Working, Status::Idle]
        );
    }
}
