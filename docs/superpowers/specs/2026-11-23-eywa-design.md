# Wooting Glance on Eywa

Agreed with Sam on 2026-11-23. Glance's agent lights are rebuilt from
scratch on Eywa, the renamed Cusco, using the data structures Eywa defines.
The earlier Cusco version (branch `feat/cusco`) worked, but it isn't the base:
this design starts from Eywa's current model without carrying over earlier
iterations.

## Why

Glance's own hooks and its datagram socket (`glance hook`, `agents.sock`,
`src/agents.rs`) duplicate what Eywa does for every program on the machine.
Glance should only turn Eywa's published state into light, decoding it with
Eywa's own types so the two can't drift.

## Goals

- `glance watch` lights each configured LED from Eywa's row for that agent,
  decoded with the `eywa-api` crate (see Eywa's
  `docs/superpowers/specs/2026-11-23-api-crate-design.md`).
- No agent tracking of its own. `glance hook`, `agents.sock`, the hook
  parsing and `scripts/agent-hooks.py` are removed.
- No hard-coded list of agents. A layout names any Eywa client, and Glance
  says so if Eywa has no client by that name.

## Non-goals

- No change to the diagnostic commands (`devices`, `inspect`, `preview`,
  `restore`), recovery, LED painting or pulse timing.
- No per-session, per-subagent or per-reason display: one state per agent app.

## Anti-goals

- Re-deriving or second-guessing Eywa's state, for example with Glance's own
  timeouts.
- An LED lit for activity Eywa can't confirm.

## Approaches

| Approach | Trade-off | Verdict |
| --- | --- | --- |
| Glance decodes with a shared `eywa-api` crate from Eywa | One definition; Eywa changes first | **Picked by Sam** |
| Glance mirrors Eywa's documented types | No Eywa change; two copies | Rejected |
| Depend on the `eywa` crate as it is | Serialize-only server internals; server dependencies | Rejected |

## Decided

- **Base.** Branch `feat/eywa` from `feat/hardware-diagnostic` (`1ca82d4`),
  where the light-bar work lives.
- **Dependency.** `eywa-api` by git from `samctensen/eywa`: the
  `feat/api-crate` branch while that PR is open, then pinned to its merge
  commit by `rev`.
- **Subscription.** Glance sends `eywa_api`'s subscribe request for path `/`
  at depth 1 to `eywa_api::default_dir()` joined with `API_SOCKET`. It keeps
  rows by id: a snapshot replaces the map, `upsert` sets a row and `remove`
  deletes one.
- **Agent names.** A layout's `agent` is an Eywa client name, such as
  `claude`, and Glance reads the row `/<agent>`. Names are lowercase ASCII
  letters, digits, `-` and `_`. The default layout is unchanged: `claude` on
  LED 1 in `#D97757` and `codex` on LED 10 in `#10A37F`. The positional form
  `glance watch LED RGB LED RGB` still assigns Claude and Codex.
- **LEDs from `eywa_api::State`.**
  - `NeedsInput` pulses (an approval, a question or MCP input).
  - `Working` is steady.
  - `Unknown`, `Idle`, `Other`, or no row at all, is off. Sam chose off for
    unknown in the earlier iteration, and that choice stays.
- **Unknown names.** After each snapshot, Glance logs once for each
  configured agent with no client row: `Eywa has no agent named <name>`.
- **Disconnected.** The LEDs go dark, Glance retries every second, and logs
  once when Eywa is lost and once when it reconnects. The watcher keeps
  running.
- **Rejected request.** An `error` message ends `glance watch` with Eywa's
  code and message.
- **Starting Eywa.** If the socket isn't answering when `glance watch`
  launches, Glance runs `eywa start` once. A later stop is respected, because
  launchd restarts crashes and a deliberate `eywa stop` should hold. A failed
  start is logged and Glance keeps waiting. It's skipped when `EYWA_DIR` is
  set, since `eywa start` refuses for a separate copy.
- **README.**
  - Setup: `cargo install --git https://github.com/samctensen/eywa`, then
    `eywa start`, `eywa autostart on` and `eywa setup`.
  - The signal table, as above.
  - Privacy points to Eywa.
  - An upgrade section for removing Glance's old `glance hook` entries.
- **Delivery** *(Sam: push and open a PR)*.
  - Push `main` and `feat/eywa` to the empty `samctensen/wooting-glance`.
  - Open one PR from `feat/eywa` into `main`. It carries the light-bar commits
    from `feat/hardware-diagnostic` as well, since `main` has only the license.
  - `feat/hardware-diagnostic` and `feat/cusco` aren't pushed.
  - Sam merges; the builder never does. *(Branch choices are my calls.)*
- **Done means:**
  - `cargo fmt --check`, `cargo test --locked` and `cargo clippy --locked
    --all-targets -- -D warnings` pass locally and in the PR's macOS CI.
  - `eywa-api` is re-pinned to the Eywa PR's merge commit. This step waits on
    Sam merging that PR.
  - Sam passes the keyboard check from a checklist: a prompt is steady, a
    permission prompt pulses, the end of a turn is off, and after
    `eywa stop` the LEDs go dark and stay dark.
  *(My calls. Sam ran the last keyboard check himself.)*
- **Sam's Mac.** Glance's hooks were removed on 2026-11-18. Before the
  keyboard check, confirm that none remain and that Eywa's hooks are
  installed. Ask Sam before changing any settings file.

## Left to the builder

- Module layout, and how the subscriber hands states to the watch loop.
- Loop timing, as long as the pulse keeps its current rate and ⌃C stays
  responsive.
