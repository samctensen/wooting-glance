# Wooting Glance

Follow Claude Code and Codex on your Wooting 80HE. Glance listens for local
agent hooks and shows Claude on LED 1 and Codex on LED 10. Blue means working,
amber means a tool requested approval, and green marks a stopped turn for up
to eight seconds. When all sessions are idle, Glance restores your saved effects.

## Run the agent lights on macOS

With a connected 80HE, run these commands from the checkout:

```sh
cargo build --release --locked
python3 scripts/agent-hooks.py
./target/release/glance watch
```

Keep the watcher running and start a fresh agent session. In Codex, use
`/hooks` to review and trust the Glance command definitions. The installer adds
entries to your user hook files and preserves existing handlers. To stop,
press Control-C in the watcher. Remove its hooks with
`python3 scripts/agent-hooks.py --remove` before moving the checkout.

The same adapters target the CLI and local coding sessions in the desktop
apps, including the Code tab in Claude Desktop. Ordinary Claude Chat, Cowork,
and remote sessions are outside this integration. Desktop hook delivery still
needs live verification. See the [Claude Code hooks](https://code.claude.com/docs/en/hooks)
and [Codex hooks](https://developers.openai.com/codex/hooks) references.

Each LED combines sessions for that service, with approval taking priority.
The other eight LEDs go dark during activity. Hook handlers forward only the
service, session identifier, and status over a local socket. A stopped watcher
misses events. An abandoned session expires after an hour without a hook.

Glance is designed to work alongside Wootility, which remains responsible for
your keyboard configuration. This is an independent project, not affiliated with
or endorsed by Wooting, OpenAI, Anthropic, or Muse.

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
