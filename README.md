# Wooting Glance

Follow Claude Code, Codex, Grok Build, and Muse Code on your Wooting 80HE. Assign agents
to individual LEDs and choose their colors.

| Signal | Meaning |
| --- | --- |
| LED 1, orange `#D97757` | Claude Code |
| LED 10, green `#10A37F` | Codex |
| Steady | The agent is working |
| Slow pulse | The agent is waiting on you: a tool approval, a question or MCP input |
| Off | Idle, or Eywa can't confirm what the agent is doing |

The defaults come from [Claude’s logo palette](https://platform.claude.com/docs/en/manage-claude/oauth-apps/branding-samples)
and the [OpenAI Developers plugin’s brand color](https://github.com/openai/plugins/blob/main/plugins/openai-developers/.codex-plugin/plugin.json).

## Run the agent lights on macOS

Glance shows what [Eywa](https://github.com/samctensen/eywa) publishes. Eywa is
a background service that tracks Claude Code, Codex, Grok Build and Muse Code;
Glance reads its state with Eywa's own `eywa-api` types. Install Eywa and
connect your agents once:

```sh
cargo install --git https://github.com/samctensen/eywa
eywa start
eywa autostart on
eywa setup
```

In Codex, run `/hooks` and trust the new hooks. In Grok Build, trust the folders
you work in. Start a new session in each agent so it loads the hooks.

Then, with a connected 80HE, run these commands from the checkout:

```sh
cargo build --release --locked
./target/release/glance watch
```

If Eywa isn't running when you start `glance watch`, Glance starts it with
`eywa start`. Keep the watcher running; to stop, press Control-C. If Eywa stops
or restarts later, the LEDs go dark until it's back. Glance reconnects on its
own, but it doesn't restart an Eywa you stopped.

### Upgrade from Glance's own hooks

Earlier versions installed `glance hook` entries in each agent's settings, and
those entries now fail on every agent event. From the same checkout, restore
the old installer, remove its hooks, then delete it again:

```sh
git show "$(git rev-list --full-history -n 1 HEAD -- scripts/agent-hooks.py)^:scripts/agent-hooks.py" > scripts/agent-hooks.py &&
  for s in claude codex grok muse; do python3 scripts/agent-hooks.py --remove --service "$s" || break; done
rm -f scripts/agent-hooks.py
```

Or delete every hook whose command ends in `glance hook <agent>` from
`~/.claude/settings.json`, `~/.codex/hooks.json`,
`~/.config/muse/settings.json` and `~/.grok/hooks/wooting-glance.json`.

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

An agent is any client Eywa reports by name: `claude`, `codex`, `grok` and
`muse` today. Glance names any configured agent Eywa doesn't have, and leaves
its LED dark. LED 1 is on the left. Assign the same agent to multiple
positions to give it more than one LED. Stop the watcher before running `glance preview` to see
the configured colors. Restart `glance watch` to load the layout. With no
config file, Glance uses the original two end LEDs. The positional command
`glance watch 2 FF8000 9 00AA88` still works for Claude and Codex.

The watcher adds the configured indicators over your saved effects. If color readback
shows the keyboard ignored the combined profile, Glance switches to a temporary
bar with the other LEDs dark. Quitting restores the saved profile.
Pause the watcher before editing the bar in Wootility or running the diagnostic.

Glance reads only Eywa's summary for each agent app, over Eywa's local socket.
See [Eywa's privacy notes](https://github.com/samctensen/eywa#privacy) for what
its hooks collect.

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
