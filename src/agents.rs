use crate::{
    device::Device,
    eywa::{self, Clients},
    layout::Indicator,
};
use anyhow::{Result, bail, ensure};
use eywa_api::State;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::RecvTimeoutError,
    },
    time::{Duration, Instant},
};
use wooting_glance::{
    protocol,
    recovery::{self, Keyboard, Recovery},
};

// Full intensity is the brand RGB value. Scale all three channels together.
const PULSE: [u16; 8] = [255, 223, 159, 96, 64, 96, 159, 223];

fn frame(layout: &[Indicator], clients: &Clients, elapsed: Duration) -> Vec<(u8, [u8; 3])> {
    let phase = (elapsed.as_millis() / 300 % PULSE.len() as u128) as usize;
    layout
        .iter()
        .map(|entry| {
            let level = match clients.get(&entry.agent) {
                Some(State::NeedsInput) => PULSE[phase],
                Some(State::Working) => 255,
                _ => 0,
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

/// Configured agents Eywa has no client for, each named only once.
fn missing<'a>(
    layout: &'a [Indicator],
    clients: &Clients,
    warned: &mut BTreeSet<String>,
) -> Vec<&'a str> {
    layout
        .iter()
        .filter(|entry| !clients.contains_key(&entry.agent) && warned.insert(entry.agent.clone()))
        .map(|entry| entry.agent.as_str())
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

/// Shows Eywa's agent states on the layout until ⌃C.
pub fn watch(
    dir: &Path,
    socket: &Path,
    start_eywa: impl Fn() -> Result<()> + Send + 'static,
    layout: &[Indicator],
) -> Result<()> {
    let recovery = Recovery::acquire(dir)?;
    let running = Arc::new(AtomicBool::new(true));
    let flag = running.clone();
    ctrlc::set_handler(move || flag.store(false, Ordering::Relaxed))?;
    let updates = eywa::subscribe(socket.to_path_buf(), Duration::from_secs(1), start_eywa);
    let device = Device::open()?;
    if dir.join("recovery.json").exists() {
        recovery::restore(&device, &device.identity, &recovery)?;
    }
    for entry in layout {
        eprintln!("Watching {} on LED {}.", entry.agent, entry.led + 1);
    }
    let start = Instant::now();
    let dark = frame(layout, &Clients::new(), Duration::ZERO);
    recovery::with_overlay(
        &device,
        &device.identity,
        &recovery,
        &dark,
        |slot, baseline| {
            let mut keep = true;
            paint(&device, slot, baseline, &dark, &mut keep)?;
            let mut clients = Clients::new();
            let mut warned = BTreeSet::new();
            let mut last = dark.clone();
            let mut last_profile_check = Instant::now();
            while running.load(Ordering::Relaxed) {
                match updates.recv_timeout(Duration::from_millis(100)) {
                    Ok(Ok(Some(update))) => {
                        for name in missing(layout, &update, &mut warned) {
                            eprintln!("Glance: Eywa has no agent named {name}.");
                        }
                        clients = update;
                    }
                    Ok(Ok(None)) => clients.clear(),
                    Ok(Err(e)) => return Err(e),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => bail!("Eywa subscriber stopped"),
                }
                let now = Instant::now();
                let leds = frame(layout, &clients, now.duration_since(start));
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{DEFAULT, parse};

    fn clients(entries: &[(&str, State)]) -> Clients {
        entries
            .iter()
            .map(|(name, state)| (name.to_string(), *state))
            .collect()
    }

    #[test]
    fn json_layout_paints_only_assigned_positions() {
        let layout = parse(r##"{"leds":{"7":{"agent":"grok","color":"#AA2200"},"2":{"agent":"grok","color":"#223344"},"10":{"agent":"codex","color":"#10A37F"}}}"##).unwrap();
        assert_eq!(
            frame(
                &layout,
                &clients(&[("grok", State::Working)]),
                Duration::ZERO
            ),
            [(1, [34, 51, 68]), (6, [170, 34, 0]), (9, [0; 3])]
        );
    }

    #[test]
    fn only_the_waiting_agent_changes_intensity() {
        let layout = parse(DEFAULT).unwrap();
        let states = clients(&[("claude", State::NeedsInput), ("codex", State::Working)]);
        let full = frame(&layout, &states, Duration::ZERO);
        let dim = frame(&layout, &states, Duration::from_millis(1200));
        assert_eq!(full, [(0, [217, 119, 87]), (9, [16, 163, 127])]);
        assert_eq!(dim[0], (0, [54, 30, 22]));
        assert_eq!(dim[1], full[1]);
    }

    #[test]
    fn unknown_other_idle_and_missing_agents_are_dark() {
        let layout = parse(DEFAULT).unwrap();
        for states in [
            clients(&[("claude", State::Unknown), ("codex", State::Other)]),
            clients(&[("claude", State::Idle)]),
            Clients::new(),
        ] {
            assert_eq!(
                frame(&layout, &states, Duration::ZERO),
                [(0, [0; 3]), (9, [0; 3])]
            );
        }
    }

    #[test]
    fn missing_agents_are_named_once() {
        let layout = parse(r##"{"leds":{"1":{"agent":"claude","color":"#000000"},"2":{"agent":"cursor","color":"#000000"},"3":{"agent":"cursor","color":"#000000"}}}"##).unwrap();
        let states = clients(&[("claude", State::Idle)]);
        let mut warned = BTreeSet::new();
        assert_eq!(missing(&layout, &states, &mut warned), ["cursor"]);
        assert!(missing(&layout, &states, &mut warned).is_empty());
    }
}
