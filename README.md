# Wooting Glance

AI activity, at a glance—on your Wooting light bar.

Glance is an experimental companion for the Wooting 80HE. The goal is to give
Claude and ChatGPT their own light-bar zones, so you can see when a service is
working or a response is ready. Each LED, color, and status pattern will be
customizable. Muse support is being investigated.

**Status:** early development. Browser integrations are not available yet.
The repository currently contains a macOS hardware diagnostic. One- and two-LED
previews and restoration have been exercised on an 80HE; full-bar control is
still being investigated.

Glance is designed to work alongside Wootility, which remains responsible for
your keyboard configuration. This is an independent project, not affiliated with
or endorsed by Wooting, OpenAI, Anthropic, or Muse.

## Direction

- Service status first, with fixed zones that stay recognizable.
- Individual LED and color controls, with useful defaults.
- Local processing; no collection of prompts or responses.
- Temporary lighting overlays that return to your saved Wootility settings.

## Try the diagnostic

Requires macOS, Rust 1.89 or later, and one connected Wooting 80HE.

```sh
cargo run --locked -- devices
cargo run --locked -- inspect
cargo run --locked -- preview 1 FF0000
cargo run --locked -- preview 1 FF8000 10 00CCFF
```

Choose one or two distinct LEDs (1–10) and a six-digit RGB color for each. LED 1
is the far-left segment. A single LED overlays the saved effects. Two LEDs use
a temporary profile at the saved brightness, with the other eight LEDs dark.
Glance checks the selected colors, waits three seconds, then uploads the saved lighting
and reads the bar again. Save pending changes in Wootility first and leave its
editor untouched during the preview. The diagnostic only changes RAM.

Preview currently requires an enabled bar with **Inherit global effects** off
and an onboard profile. Disabled, inherited, and linked profiles are not yet
supported. A ten-color frame was ignored during testing; simultaneous control
of all ten LEDs is not claimed by this version.

If the process is killed or the keyboard is disconnected mid-preview, reconnect
it and run:

```sh
cargo run --locked -- restore
```

Recovery state lives in `~/Library/Application Support/wooting-glance/`, outside
the repository. It records the device identity and saved bar configuration before
any preview write. Commands share a lock to prevent concurrent changes. Recovery
uses the keyboard's latest saved lighting and keeps the record if upload or
readback fails. Readback is a color snapshot, not proof that an animated effect
matches at every instant. Other RGB applications may interfere.

## Development

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Protocol framing, malformed replies, color validation, restoration failures, and
concurrent-command protection are tested without a keyboard. CI runs on macOS.
Protocol attribution is in [third-party notices](THIRD_PARTY_NOTICES.md).

## License

[MIT](LICENSE). Created by [Sam Christensen](https://github.com/samctensen).
If you share something built with Glance, a link back is appreciated.
