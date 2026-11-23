use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Clone, Debug, PartialEq)]
pub struct Indicator {
    pub led: u8,
    pub color: [u8; 3],
    /// An Eywa client name, such as `claude`: Glance reads the row `/<agent>`.
    pub agent: String,
}

/// Whether `name` can be an Eywa client name.
pub fn valid_agent(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    agent: String,
    color: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    leds: BTreeMap<String, Entry>,
}

pub const DEFAULT: &str = r##"{
  "leds": {
    "1": { "agent": "claude", "color": "#D97757" },
    "10": { "agent": "codex", "color": "#10A37F" }
  }
}
"##;

pub fn parse(contents: &str) -> Result<Vec<Indicator>> {
    let config: Config = serde_json::from_str(contents)?;
    ensure!(!config.leds.is_empty(), "assign at least one LED");
    let mut result = Vec::new();
    for (position, entry) in config.leds {
        let (led, color) = super::parse_led(&position, &entry.color)
            .with_context(|| format!("invalid entry for LED {position}"))?;
        ensure!(
            position == (led + 1).to_string(),
            "invalid LED key {position}"
        );
        ensure!(
            valid_agent(&entry.agent),
            "invalid agent name {:?} for LED {position}",
            entry.agent
        );
        result.push(Indicator {
            led,
            color,
            agent: entry.agent,
        });
    }
    result.sort_by_key(|indicator| indicator.led);
    Ok(result)
}

pub fn load(path: &Path) -> Result<Vec<Indicator>> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            parse(&contents).with_context(|| format!("invalid layout in {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => parse(DEFAULT),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_are_eywa_client_names() {
        let layout = parse(r##"{"leds":{"3":{"agent":"cursor-2","color":"#000000"}}}"##).unwrap();
        assert_eq!(layout[0].agent, "cursor-2");
        for name in ["Claude", "", "my agent", "/claude"] {
            let config = format!(r##"{{"leds":{{"3":{{"agent":"{name}","color":"#000000"}}}}}}"##);
            assert!(parse(&config).is_err(), "{name:?} was accepted");
        }
    }
}
