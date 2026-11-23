// Light-bar framing and canvas layout adapted from WootingLangBar.
// See THIRD_PARTY_NOTICES.md for the original MIT notice and source.
use anyhow::{Result, ensure};
use prost::Message;

pub const LED_COUNT: usize = 10;

#[derive(Clone, PartialEq, Message)]
pub struct BarProfile {
    #[prost(bool, tag = "1")]
    pub enabled: bool,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub layers: Vec<Vec<u8>>,
    #[prost(bool, tag = "3")]
    pub inherits: bool,
    #[prost(uint32, optional, tag = "4")]
    pub brightness: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
struct Canvas {
    // Firmware requires presence even for the zero-indexed first LED.
    #[prost(uint32, optional, tag = "1")]
    start: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    end: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
struct Solid {
    #[prost(uint32, tag = "1")]
    argb: u32,
    #[prost(message, optional, tag = "2")]
    canvas: Option<Canvas>,
}

#[derive(Clone, PartialEq, Message)]
struct Layer {
    #[prost(message, optional, tag = "6")]
    solid: Option<Solid>,
}

#[derive(Clone, PartialEq, Message)]
struct Overlay {
    #[prost(message, repeated, tag = "2")]
    layers: Vec<Layer>,
}

#[derive(Clone, PartialEq, Message)]
struct Rgb {
    #[prost(uint32, tag = "1")]
    r: u32,
    #[prost(uint32, tag = "2")]
    g: u32,
    #[prost(uint32, tag = "3")]
    b: u32,
}

#[derive(Clone, PartialEq, Message)]
struct BarColors {
    #[prost(message, repeated, tag = "1")]
    colors: Vec<Rgb>,
}

pub fn response(report: &[u8], command: u8) -> Result<Option<&[u8]>> {
    if report.len() < 4 || report[1..4] != [0xd1, 0xda, command] {
        return Ok(None);
    }
    ensure!((1..=6).contains(&report[0]), "unexpected input report ID");
    ensure!(report.len() >= 7, "truncated response header");
    ensure!(
        report[4] == 0x88,
        "command {command} returned status {:#04x}",
        report[4]
    );
    let len = u16::from_le_bytes([report[5], report[6]]) as usize;
    ensure!(len <= report.len() - 7, "truncated response body");
    Ok(Some(&report[7..7 + len]))
}

/// The diagnostic initially supports saved, enabled, non-inherited profiles only.
pub fn baseline(stored: &[u8]) -> Result<BarProfile> {
    ensure!(
        stored.len() <= 2040,
        "profile exceeds the largest HID report"
    );
    let profile = BarProfile::decode(stored)?;
    ensure!(
        profile.enabled,
        "preview requires an enabled saved light-bar profile"
    );
    ensure!(
        !profile.inherits,
        "preview of inherited light-bar profiles is not supported yet"
    );
    Ok(profile)
}

pub fn preview(stored: &[u8], leds: &[(u8, [u8; 3])]) -> Result<Vec<u8>> {
    bar_frame(stored, leds, leds.len() == 1)
}

/// Append indicators to saved effects when requested, keeping unknown fields.
pub fn bar_frame(stored: &[u8], leds: &[(u8, [u8; 3])], keep_effects: bool) -> Result<Vec<u8>> {
    let saved = baseline(stored)?;
    ensure!(
        (1..=LED_COUNT).contains(&leds.len()),
        "choose one to ten LEDs"
    );
    let mut seen = [false; LED_COUNT];
    let mut layers = Vec::with_capacity(leds.len());
    for &(index, rgb) in leds {
        ensure!((index as usize) < LED_COUNT, "LED index out of range");
        ensure!(!seen[index as usize], "duplicate LED index");
        seen[index as usize] = true;
        layers.push(Layer {
            solid: Some(Solid {
                argb: u32::from_be_bytes([255, rgb[0], rgb[1], rgb[2]]),
                canvas: Some(Canvas {
                    start: Some(index.into()),
                    end: Some(index.into()),
                }),
            }),
        });
    }
    let mut body = if !keep_effects {
        // The indicators own the bar during preview. Keep the saved brightness.
        let mut frame = BarProfile {
            enabled: true,
            layers: vec![],
            inherits: false,
            brightness: saved.brightness,
        }
        .encode_to_vec();
        frame.extend([24, 0]); // Explicit inheritGlobal=false, including field presence.
        frame
    } else {
        stored.to_vec()
    };
    body.extend(Overlay { layers }.encode_to_vec());
    ensure!(body.len() <= 2040, "overlay exceeds the largest HID report");
    Ok(body)
}

/// Output report 32, namespace zero, RAM only. No API accepts a save flag.
pub fn upload(slot: u8, body: &[u8]) -> Result<Vec<u8>> {
    ensure!(slot < 4, "only onboard profile slots 0–3 are supported");
    let sizes = [32, 62, 254, 510, 1022, 2046];
    let (index, &size) = sizes
        .iter()
        .enumerate()
        .find(|(_, size)| body.len() <= **size - 6)
        .ok_or_else(|| anyhow::anyhow!("profile exceeds the largest HID report"))?;
    let mut report = vec![0; size + 1];
    report[..5].copy_from_slice(&[index as u8 + 1, 0xd1, 0xda, 32, slot << 1]);
    report[5..7].copy_from_slice(&(body.len() as u16).to_le_bytes());
    report[7..7 + body.len()].copy_from_slice(body);
    Ok(report)
}

pub fn colors(body: &[u8]) -> Result<Vec<[u8; 3]>> {
    let frame = BarColors::decode(body)?;
    ensure!(
        frame.colors.len() == LED_COUNT,
        "expected exactly ten light-bar colors"
    );
    frame
        .colors
        .into_iter()
        .map(|c| Ok([c.r.try_into()?, c.g.try_into()?, c.b.try_into()?]))
        .collect()
}

pub fn verify_colors(actual: &[[u8; 3]], expected: &[(u8, [u8; 3])]) -> Result<()> {
    ensure!(
        actual.len() == LED_COUNT,
        "expected ten colors in preview readback"
    );
    for &(index, color) in expected {
        ensure!(
            actual.get(index as usize) == Some(&color),
            "LED {} readback does not match the requested color",
            index as usize + 1
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_selected_leds_and_rejects_mismatched_readback() {
        let actual = vec![[255, 0, 0]; 10];
        assert!(verify_colors(&actual, &[(9, [255, 0, 0])]).is_ok());
        assert!(verify_colors(&actual, &[(9, [0, 255, 0])]).is_err());
        assert!(verify_colors(&actual, &[(10, [255, 0, 0])]).is_err());
        assert!(verify_colors(&[], &[(0, [255, 0, 0])]).is_err());
    }

    #[test]
    fn response_checks_identity_status_and_length() {
        assert_eq!(
            response(&[1, 0xd1, 0xda, 68, 0x88, 1, 0, 42, 0], 68).unwrap(),
            Some(&[42][..])
        );
        assert_eq!(
            response(&[1, 0xd1, 0xda, 11, 0x88, 0, 0], 68).unwrap(),
            None
        );
        assert!(response(&[1, 0xd1, 0xda, 68, 0xff, 0, 0], 68).is_err());
        assert!(response(&[1, 0xd1, 0xda, 68, 0x88, 2, 0, 1], 68).is_err());
        assert!(response(&[1, 0xd1, 0xda, 68], 68).is_err());
        assert_eq!(response(&[0, 0, 0], 68).unwrap(), None);
    }

    #[test]
    fn upload_selects_smallest_report_and_never_saves() {
        for (len, id, size) in [
            (26, 1, 33),
            (27, 2, 63),
            (56, 2, 63),
            (57, 3, 255),
            (248, 3, 255),
            (249, 4, 511),
            (504, 4, 511),
            (505, 5, 1023),
            (1016, 5, 1023),
            (1017, 6, 2047),
            (2040, 6, 2047),
        ] {
            for slot in 0..4 {
                let report = upload(slot, &vec![42; len]).unwrap();
                assert_eq!(report.len(), size);
                assert_eq!(report[0], id);
                assert_eq!(&report[1..5], &[0xd1, 0xda, 32, slot << 1]);
                assert_eq!(report[4] & 1, 0);
                assert_eq!(u16::from_le_bytes([report[5], report[6]]) as usize, len);
                assert!(report[7 + len..].iter().all(|b| *b == 0));
            }
        }
        assert!(upload(4, &[0]).is_err());
        assert!(upload(0, &vec![0; 2041]).is_err());
    }

    #[test]
    fn preview_preserves_base_and_unknown_fields_and_encodes_one_led() {
        // Enabled, local brightness 128, unknown field 10. Keep the bytes intact.
        let stored = [8, 1, 32, 128, 1, 80, 7];
        let body = preview(&stored, &[(4, [255, 0, 0])]).unwrap();
        assert!(body.starts_with(&stored));
        assert_eq!(
            &body[stored.len()..],
            &[
                18, 14, 50, 12, 8, 128, 128, 252, 255, 15, 18, 4, 8, 4, 16, 4
            ]
        );
        assert!(preview(&stored, &[(10, [0, 0, 0])]).is_err());
        assert!(preview(&stored, &[(0, [0, 0, 0]), (0, [1, 1, 1])]).is_err());
        assert!(preview(&stored, &[]).is_err());
    }

    #[test]
    fn pair_uses_only_the_selected_layers_and_retains_brightness() {
        let stored = [8, 1, 18, 0, 32, 128, 1];
        let body = preview(&stored, &[(0, [255, 128, 0]), (9, [0, 204, 255])]).unwrap();
        let frame = BarProfile::decode(body.as_slice()).unwrap();
        assert_eq!(frame.brightness, Some(128));
        assert_eq!(frame.layers.len(), 2);
        for (layer, index, color) in [
            (&frame.layers[0], 0, 0xffff8000),
            (&frame.layers[1], 9, 0xff00ccff),
        ] {
            let solid = Layer::decode(layer.as_slice()).unwrap().solid.unwrap();
            assert_eq!(solid.argb, color);
            let canvas = solid.canvas.unwrap();
            assert_eq!((canvas.start, canvas.end), (Some(index), Some(index)));
        }
    }

    #[test]
    fn first_led_encodes_explicit_zero_canvas_bounds() {
        let body = preview(&[8, 1], &[(0, [255, 0, 0])]).unwrap();
        assert_eq!(
            &body[2..],
            &[
                18, 14, 50, 12, 8, 128, 128, 252, 255, 15, 18, 4, 8, 0, 16, 0
            ]
        );
    }

    #[test]
    fn preview_refuses_unsupported_or_malformed_baselines() {
        for stored in [
            vec![],
            vec![8, 0],
            vec![8, 1, 24, 1],
            vec![8, 1, 18, 10],
            vec![8, 128],
            vec![0],
        ] {
            assert!(
                preview(&stored, &[(0, [1, 2, 3])]).is_err(),
                "accepted {stored:?}"
            );
        }
    }

    #[test]
    fn color_readback_requires_ten_valid_rgb_triplets() {
        let pixel = [10, 6, 8, 255, 1, 24, 128, 1];
        assert_eq!(colors(&pixel.repeat(10)).unwrap(), vec![[255, 0, 128]; 10]);
        assert!(colors(&pixel.repeat(9)).is_err());
        assert!(colors(&[10, 10, 8]).is_err());
        let overflow = [10, 3, 8, 128, 2];
        assert!(colors(&overflow.repeat(10)).is_err());
    }
}
