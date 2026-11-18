use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Service {
    Claude,
    Codex,
    Grok,
}

pub const COUNT: usize = Service::Grok as usize + 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Indicator {
    pub led: u8,
    pub color: [u8; 3],
    pub agent: Service,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    agent: Service,
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
