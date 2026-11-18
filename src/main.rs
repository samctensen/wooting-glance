#[cfg(target_os = "macos")]
mod agents;
#[cfg(target_os = "macos")]
mod device;

use anyhow::{Context, Result, bail, ensure};
use std::io::Write;

const HELP: &str = "Wooting Glance — experimental 80HE hardware diagnostic

glance watch                  Show local agent activity until interrupted
glance hook claude|codex       Receive a local coding-agent hook on stdin
glance devices                List supported keyboards (read-only)
glance inspect                Read active profile and ten LED colors
glance preview LED RGB [LED RGB]  Preview one or two LEDs for three seconds
glance restore                Recover saved lighting after an interrupted preview

Recovery is stored in ~/Library/Application Support/wooting-glance/.
With two LEDs the other eight go dark for the preview.
Before preview: save changes in Wootility and leave its editor untouched.
Preview currently requires an enabled, non-inherited onboard bar profile.
No command saves lighting to the keyboard's flash memory.";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Devices,
    Watch,
    Hook(String),
    Inspect,
    Preview(Vec<(u8, [u8; 3])>),
    Restore,
}

fn parse(args: &[String]) -> Result<Command> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    Ok(match args.as_slice() {
        [] | ["help" | "--help" | "-h"] => Command::Help,
        ["watch"] => Command::Watch,
        ["hook", service @ ("claude" | "codex")] => Command::Hook((*service).into()),
        ["devices"] => Command::Devices,
        ["inspect"] => Command::Inspect,
        ["restore"] => Command::Restore,
        ["preview", led, color] => Command::Preview(vec![parse_led(led, color)?]),
        ["preview", first, first_color, second, second_color] => {
            let a = parse_led(first, first_color)?;
            let b = parse_led(second, second_color)?;
            ensure!(a.0 != b.0, "choose two different LEDs");
            Command::Preview(vec![a, b])
        }
        _ => bail!("unknown command or arguments; run `glance --help`"),
    })
}

fn parse_led(led: &str, color: &str) -> Result<(u8, [u8; 3])> {
    let led: u8 = led.parse()?;
    ensure!((1..=10).contains(&led), "LED must be from 1 to 10");
    let color = color.strip_prefix('#').unwrap_or(color);
    ensure!(
        color.len() == 6 && color.bytes().all(|b| b.is_ascii_hexdigit()),
        "color must contain six hexadecimal digits"
    );
    let word = u32::from_str_radix(color, 16)?.to_be_bytes();
    Ok((led - 1, [word[1], word[2], word[3]]))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Glance: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let command = parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if command == Command::Help {
        println!("{HELP}");
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    return macos(command);
    #[cfg(not(target_os = "macos"))]
    bail!("the hardware diagnostic currently supports macOS only");
}

#[cfg(target_os = "macos")]
fn macos(command: Command) -> Result<()> {
    use prost::Message;
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };
    use wooting_glance::{
        protocol::BarProfile,
        recovery::{self, Keyboard, Recovery},
    };

    if command == Command::Devices {
        return device::list();
    }
    let home = std::env::var_os("HOME")
        .context("HOME is unavailable; cannot locate recovery directory")?;
    let state_dir = PathBuf::from(home).join("Library/Application Support/wooting-glance");
    if let Command::Hook(service) = &command {
        return agents::forward(&state_dir, service);
    }
    if command == Command::Watch {
        return agents::watch(&state_dir);
    }
    let recovery = Recovery::acquire(&state_dir)?;
    let keyboard = device::Device::open()?;
    match command {
        Command::Inspect => {
            let slot = keyboard.active_slot()?;
            let stored = keyboard.stored_profile(slot)?;
            let profile = BarProfile::decode(stored.as_slice())?;
            println!(
                "Wooting 80HE · shared HID connection · onboard slot {}",
                slot + 1
            );
            println!(
                "Saved bar: enabled={}, inherits={}, brightness={:?}, layers={}",
                profile.enabled,
                profile.inherits,
                profile.brightness,
                profile.layers.len()
            );
            print_colors("Current bar", &keyboard.colors()?)?;
        }
        Command::Preview(leds) => {
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = cancelled.clone();
            ctrlc::set_handler(move || {
                signal.store(true, Ordering::SeqCst);
            })?;
            println!("Previewing for three seconds; saved lighting will be reapplied.");
            let restored =
                recovery::preview(&keyboard, &keyboard.identity, &recovery, &leds, || {
                    thread::sleep(Duration::from_millis(150));
                    let actual = keyboard.colors()?;
                    print_colors("Preview readback", &actual)?;
                    wooting_glance::protocol::verify_colors(&actual, &leds)?;
                    let deadline = Instant::now() + Duration::from_secs(3);
                    while Instant::now() < deadline {
                        if cancelled.load(Ordering::SeqCst) {
                            bail!("preview interrupted");
                        }
                        thread::sleep(Duration::from_millis(25));
                    }
                    Ok(())
                })?;
            writeln!(
                std::io::stdout().lock(),
                "Saved lighting uploaded; readback received; recovery record cleared."
            )?;
            print_colors("Restored readback", &restored)?;
        }
        Command::Restore => {
            let restored = recovery::restore(&keyboard, &keyboard.identity, &recovery)?;
            writeln!(
                std::io::stdout().lock(),
                "Saved lighting uploaded; readback received; recovery record cleared."
            )?;
            print_colors("Restored readback", &restored)?;
        }
        Command::Help | Command::Devices | Command::Hook(_) | Command::Watch => unreachable!(),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn print_colors(label: &str, colors: &[[u8; 3]]) -> Result<()> {
    let colors = colors
        .iter()
        .map(|c| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]))
        .collect::<Vec<_>>()
        .join(" ");
    writeln!(std::io::stdout().lock(), "{label}: {colors}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(words: &[&str]) -> Result<Command> {
        parse(&words.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn parses_led_and_color_without_accepting_extra_arguments() {
        assert_eq!(
            command(&["preview", "10", "#FF0080"]).unwrap(),
            Command::Preview(vec![(9, [255, 0, 128])])
        );
        for args in [
            vec!["preview", "0", "ff0000"],
            vec!["preview", "11", "ff0000"],
            vec!["preview", "1", "💜ab"],
            vec!["preview", "1", "12345g"],
            vec!["restore", "extra"],
            vec!["devices", "extra"],
        ] {
            assert!(command(&args).is_err());
        }
    }
    #[test]
    fn preview_accepts_two_distinct_leds() {
        assert_eq!(
            command(&["preview", "1", "FF8000", "10", "00CCFF"]).unwrap(),
            Command::Preview(vec![(0, [255, 128, 0]), (9, [0, 204, 255])])
        );
        assert!(command(&["preview", "1", "ff0000", "1", "ffffff"]).is_err());
    }
    #[test]
    fn no_arguments_show_help_and_unsupported_patterns_are_rejected() {
        assert_eq!(command(&[]).unwrap(), Command::Help);
        assert!(command(&["preview", "rainbow"]).is_err());
    }
}
