# Changelog

All notable changes to cyrup are recorded here. Each released version gets a `## x.y.z` section;
the newest is first. `/changelog` renders this file inside the agent, and a new entry raises the
"What's New" notice on the next start.

## Unreleased

### The herdr pane status feed now runs by default (HERDR-010)

`cyrup` inside [herdr](https://github.com/herdr) now reports its pane to herdr's Agents sidebar in
every session that runs inside a herdr pane — `idle`, `working` while a turn runs, and `blocked`
while a permission, MCP, intercom or flux dialog is waiting on you — and releases the row on exit.

It previously did none of that unless you had opted into subagent delegation. The whole feed lived
inside the subagents extension's crate, and that extension is opt-in and default OFF
(`CYRUP_SUBAGENTS`, or a `subagents/config.json`). In a default session the extension never
attached, the arm was never reached, and a cyrup sitting in a pane with a live herdr server
reported nothing at all. No log line said so: the code that would have logged it lived in the
extension that was not loaded.

- **New crate `cyrup-ext-herdr`** — the state machine, the reporter task, the event consumer, the
  opt-in sidebar re-ordering, and a built-in (`herdr-status`) attached next to `mcp` and
  `permission-system`. Its only gate is "inside a herdr pane, with a UI".
- Three switches, like every other built-in: `"extensions": ["-builtin:herdr-status"]` in settings,
  `CYRUP_HERDR_STATUS=0` in the environment, and `--no-extensions`, which drops it the way it drops
  the other ambient built-ins (`-e builtin:herdr-status` loads it back). The sidebar *re-ordering*
  stays opt-in at `CYRUP_HERDR_AGENT_VIEW=1`, because that touches panes cyrup does not own.
- The pane's release now survives a hung herdr. The `SessionShutdown` handler *spawns* the release
  and bounds only the join: a 5 s release inside a 5 s dispatcher budget used to consume the whole
  budget on its first statement, so the handler future was dropped and every shutdown handler
  registered after it never ran — silently.
- Subagents now **feeds** the bridge a fleet snapshot instead of the bridge reaching into the fleet,
  and stops driving the root-turn edge, which the built-in owns. The privacy rule — raw prompts and
  child output never reach pane metadata — moved with the renderer and is still pinned.
- The reports' authority is renamed `cyrup:subagents` → `cyrup:status`.

## 0.1.0 - 2026-10-11

The first versioned pre-release. Development began 2026-08-27; this entry covers 521 commits
across 149 merged pull requests.

cyrup is a coding agent in Rust: one static binary, 40 model providers, and extensions that run as
sandboxed WebAssembly components. It follows the design of the
[Pi](https://github.com/earendil-works/pi) agent harness — a minimal core, everything-is-an-extension,
an agent that can extend itself — rebuilt on a Rust backbone across 33 crates.

### Working end to end

- The agent loop, the provider layer and the tool set.
- The session tree, with fork, clone and resume.
- The terminal interface, in both the inline viewport and the alternate screen.
- The extension host, with native and WebAssembly guests.
- All five run modes.
- The MCP client and the ACP adapter.
- The `workflowScript` runtime.
- Background and remote subagent delegation.
- The herdr integration.

### Parity method

cyrup is a port, so its behaviour is adjudicated against its upstreams rather than designed from
scratch. Divergences are tracked as a ledger of behavioural differences under
[`docs/gap-analysis/`](docs/gap-analysis), each entry carrying the upstream citation it was measured
against and the test that proves its closure. 1,124 entries are closed. Where cyrup and an upstream
disagree, the upstream is treated as correct unless the divergence is recorded, with a reason, as a
`[CYRUP-DELTA]` in the source.

Upstream citations in the source name the exact TypeScript file and line each behaviour came from,
so a reader can check the port against the original.
