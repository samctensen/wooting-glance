# Wooting Glance

Follow Claude Code, Codex, Grok Build, and Muse Code on your Wooting 80HE. Assign agents
to individual LEDs and choose their colors.

| Signal | Meaning |
| --- | --- |
| LED 1, orange `#D97757` | Claude Code |
| LED 10, green `#10A37F` | Codex |
| Steady | At least one parent or child is working |
| Slow pulse | An agent requested tool permission |
| Off | All observed agents for that service have stopped |

The defaults come from [Claude’s logo palette](https://platform.claude.com/docs/en/manage-claude/oauth-apps/branding-samples)
and the [OpenAI Developers plugin’s brand color](https://github.com/openai/plugins/blob/main/plugins/openai-developers/.codex-plugin/plugin.json).

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
and remote sessions are outside this integration. Parent and child events use the [Claude Code hooks](https://code.claude.com/docs/en/hooks)
and [Codex hooks](https://developers.openai.com/codex/hooks) interfaces.

### Add Grok Build or Muse Code

Run `python3 scripts/agent-hooks.py --service grok` after building. This installs
local hooks for Grok Build, the engine used by the community Grok.app desktop client.
Start a fresh Grok Build session after setup. To remove the hooks, add `--remove`.
[Supported Grok hook events](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md).

For Muse Code, run `python3 scripts/agent-hooks.py --service muse` and start a fresh
terminal session. The installer adds hooks to your Muse user settings.
[Muse Code hook events](https://meta-models.github.io/muse-code-sdk/1.4/guides/plugins/reference/hook-events/).

### Choose your layout

Run `./target/release/glance config init` once and edit
`~/Library/Application Support/wooting-glance/config.json`. For example:

```json
{
  "leds": {
    "1":  { "agent": "claude", "color": "#D97757" },
    "4":  { "agent": "grok",  "color": "#FFFFFF" },
    "7":  { "agent": "muse",  "color": "#A855F7" },
    "10": { "agent": "codex", "color": "#10A37F" }
  }
}
```

LED 1 is on the left. Assign the same agent to multiple positions to give it
more than one LED. Stop the watcher before running `glance preview` to see
the configured colors. Restart `glance watch` to load the layout. With no
config file, Glance uses the original two end LEDs. The positional command
`glance watch 2 FF8000 9 00AA88` still works for Claude and Codex.

The watcher adds the configured indicators over your saved effects. If color readback
shows the keyboard ignored the combined profile, Glance switches to a temporary
bar with the other LEDs dark. Quitting restores the saved profile.
Pause the watcher before editing the bar in Wootility or running the diagnostic.

Hooks forward only service, session and child identifiers and state over a local
socket. An approval indication clears on observed tool progress or completion.
Glance misses events while stopped and expires stale actors after an hour.

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
