# Lean fork plan

Baseline: `main` @ `c5c0d1e6`. Working tree has `.github/workflows/ci.yml` (new) and this file. No
code changed yet.

Scope: remove codemode, workflowScript, and subagents. Keep herdr and MCP. **No feature flags — code
is deleted, not gated.**

---

## Decisions

| # | Decision | Status |
|---|---|---|
| D1 | V8 (`deno_core`) goes when **both** JS consumers are gone: the codemode sandbox *and* workflowScript. Removing only codemode does not remove V8. | settled |
| D2 | `cyrup-codemode` is **not** codemode-only. It hosts BM25 tool search + ECMAScript string/number/key semantics used by `cyrup-mcp` and `cyrup-tool-search`. Rename to **`cyrup-tool-search-core`**, do not delete. | settled |
| D3 | The 7 herdr pane tool verbs (`inspector.*`, `project.*`) are unwanted. They are actions of the single `subagent` tool, and default-off (`CYRUP_SUBAGENTS`), so they are invisible in practice. | settled |
| D4 | herdr's **Agents sidebar status feed** is a cyrup feature that lives inside `cyrup-ext-subagents/src/herdr/`. Deleting that crate removes the sidebar. | settled |
| D5 | Delete `cyrup-ext-subagents` in place. Rebuilding the sidebar as a WASM guest extension is a **separate, optional follow-up**, not a precondition. | settled |
| D6 | Keep `wasm-host` / wasmtime. The flag cannot be turned off today without code changes (EXT-026), so there is no cheap win there. | settled |

---

## Implications

**Build size — this is not where the 29 GB is.** Measured `target/`:

- `target/debug/incremental/` — **9.7 GB**, 262 stale dirs. Reclaimable with one `rm -rf`.
- wasmtime/cranelift/wit-parser family — **2.4 GB**. Only removable by deleting the WASM host, which D6 keeps.
- V8 — ~1.2 GB (4 feature-hash copies of `libv8` + 2 × `librusty_v8.a`). This is the only piece this plan removes.
- Downloaded deps (`~/.cargo/registry`) are 2.3 GB, not 20 GB.

So: deleting codemode + workflows + subagents buys ~1.2 GB of `deps/` plus LOC/link work.

**wasmtime:** needed only for `.wasm` guest components. Native built-in extensions (MCP client,
permission gate, intercom, tool search, subagents, Flux) are feature-independent and survive without
it. But `wasm-host` is default-on in `cyrup-ext` and both `cyrup-session-svc` and `cyrup-tui` pull it
in unconditionally — `cyrup-tui/src/` cannot compile without `cyrup_ext::host::HostServices`. A
genuinely native-only graph is EXT-026, not a flag flip. Consequence: keeping WASM costs nothing extra
right now.

**herdr:** `cyrup-herdr` is a clean leaf and survives everything. Pi gets its sidebar from herdr's
installed extension `herdr-agent-state.ts`; cyrup instead ported that extension into Rust host code
inside `cyrup-ext-subagents`. cyrup cannot load a `.ts` extension — discovery wants a dir with
`extension.json` + a prebuilt `.wasm`, or a bare `*.wasm`. Separate herdr feature that is *not* the
status feed: intercom's pane-location labelling. It dies with intercom, not with subagents.

---

## Steps

**1. Delete the codemode tool.** Remove `cyrup-codemode-runtime`, the `__codemode-sandbox` subcommand,
its tests, and its dependency entries in `cyrup` / `cyrup-session-svc` / `cyrup-tui` / `cyrup-it`.
Self-contained; does not touch herdr. V8 remains after this step.

**2. Delete workflowScript.** Remove `cyrup-workflow-runtime` and `ext-subagents/src/workflows/scripted/`,
including the `build.rs` step that compiles the V8 startup snapshot, and the `deno_core` / `deno_ast` /
`deno_error` deps. **This is the step that removes V8.**

**3. Rename `cyrup-codemode` → `cyrup-tool-search-core`.** Do it *after* step 1 so only the live
modules move (`js`, `rank`, `identifier`, `source`; drop `declarations`, `discovery`, `output`, `types`
with step 1). Keep the rename commit free of deletions so `git log --follow` survives.

**4. Delete subagents.** Remove `cyrup-ext-subagents` and its surfaces: the `__subagent-runner` and
`__subagent-inspector` subcommands, the `cyrup-it` integration target and its fixture entries in
`cyrup-it/build.rs`, and the dependency entries in `cyrup`, `cyrup-intercom`,
`cyrup-permission-system`, `cyrup-herdr`. `cyrup-intercom` is the only non-test crate that depends on
it, so it needs rewiring. Accepted consequence: no status in herdr's Agents sidebar.

---

## Status

**Step 1 — DONE on `lean/1-codemode`.** `cyrup-codemode-runtime`, the `__codemode-sandbox` hop, the
tool's globals, the TUI renderer seam, the `cyrup-it` `codemode_wire` seam and the dependency entries
in `cyrup` / `cyrup-session-svc` / `cyrup-tui` / `cyrup-it` are gone; ~32k lines deleted. V8 is still
in the graph — `cyrup-ext-subagents`'s `workflows/scripted/` pulls `deno_core` until step 2.

What the step touched beyond the plan's "self-contained" line:

- **Settings.** `codemode.mode` and `codemode.inlineBudget` are deleted from `cyrup-config` (types,
  `EffectiveSettings` getters, the generated schema and its committed
  `schemas/settings.schema.json`). The settings document is not closed, so a file still carrying the
  `codemode` object loads and ignores it.
- **System prompt.** pi's `When asked about: codemode scripts … (docs/codemode.md)` docs pointer is
  dropped from the `docs` section; `cyrup-session` and `cyrup-test-support` now pin that no
  `When asked about:` entry and no `codemode` mention survive.
- **Permission labels.** `gate::format_nested_origin_label`'s `"(from codemode script)"` arm is gone:
  every nested caller is labelled `(from a nested call of tool '<name>')`, and the headless block hint
  reads `A nested call cannot answer an approval prompt without a UI: …`.
- **Tests kept, by porting them to the model's own tool call** (they were written through a script):
  `cyrup/src/session_launch/mcp_tests.rs` (11 MCP-over-real-launch-wiring tests),
  `cyrup-it` `session_svc/wasm_guest_tool_surface.rs` (5 guest-component seam tests; the structured
  result is read off the `tool_execution_end` event, which is where a host reads it), and the
  `--tools` / `--exclude-tools` / permission-gate probes in `session_launch.rs` (they now drive `bash`
  directly and prove a command ran by its marker file). Deleted as unrecoverable: the codemode-script
  half of `cyrup-it` `llama/classify.rs` (CODE-023, it tested the script's `models.classify()` global)
  and `codemode.mode: only` cases, the setting being gone.
- **Accepted deltas.** Two properties were only observable from a script: a `directTools: "search"`
  server's tool being callable without being active, and pi's script-side wait for a starting server
  being longer than the dispatcher's 5 s. The MCP seam tests now pin the fork's behaviour instead — a
  registered-but-inactive tool is unreachable to a call, and a call to a still-starting server rides
  the launch wiring's own wait (fixture server: 1 s).
- **Docs.** `docs/codemode.md` and `docs/guide/guides/codemode.md` deleted, and with them the SUMMARY
  entry; README, cli, settings, troubleshooting, tools-and-permissions, scripting, tui, zed-acp,
  llama-cpp, subagents and permissions prose updated. `ToolExposure::Codemode` stays in `cyrup-core`
  on purpose: it is a tool *exposure* that `tool_search` and the MCP adapter use, not the deleted tool
  (and `cyrup-codemode` itself stays, per D2, until the step 3 rename).

**Step 2 — absorbed by step 4, not run separately.** The plan assumed workflowScript could be removed
on its own. It cannot be removed *usefully*: `cyrup-workflow-runtime` had exactly one consumer,
`cyrup-ext-subagents/src/workflows/scripted/`, and its `build.rs` compiled the V8 startup snapshot for
that crate alone. Deleting the scripted runtime would have kept a 1,500-file crate alive to host
nothing. So step 2's payload — `cyrup-workflow-runtime`, `workflows/scripted/`, the snapshot build
step, and the `deno_core` / `deno_ast` / `deno_error` and `fancy-regex` dependency entries — went with
step 4. **V8 is now out of the graph entirely**, which is what step 2 existed to buy.

**Step 3 — not done.** `cyrup-codemode` still carries its old name. It survives step 1 for the reason
D2 gives (BM25 ranking + the ECMAScript string/number/key semantics shared by `cyrup-mcp` and
`cyrup-tool-search`). The rename is still a standalone commit, kept free of deletions so
`git log --follow` survives.

**Step 4 — DONE on `lean/4-subagents`.** `cyrup-ext-subagents` (451,714 lines) and
`cyrup-workflow-runtime` are deleted, along with `deno_core`/`deno_ast`/`deno_error`/`fancy-regex`.
Beyond the plan's list:

- **The CLI hops.** `__subagent-runner` and `__subagent-inspector` are gone from `predispatch`'s
  `Internal` enum, their dispatch arms and `set_process_name` calls are gone from `main.rs`, and
  `subagent_config.rs` / `subagent_runner_cmd.rs` / `subagent_inspector_cmd.rs` are deleted. Three
  internal subcommands remain: `__intercom-broker`, `__mcp-keyring-helper`, `__acp-terminal-login`.
  `predispatch`'s gate count went seven → five.
- **`cyrup-permission-system` keeps its semantics, by vendoring ~750 lines.** It was not a mere
test dependency: the gate's ask-forwarding and its parent-session anchor read process liveness, PID
namespace scope, start-time identity, hostname, and the safe-path-token validators out of
`cyrup-ext-subagents`. Those primitives were lifted into `cyrup-permission-system/src/primitives/`
unchanged — liveness probe, `OwnerStamp`/`ProcessStartIdentity`/`owner_demonstrably_gone`, the
published-parent-session anchor register, `validate_safe_token`/`validate_contains_root` (now a local
`PathTokenError` instead of `SubagentError`), `machine_hostname`, the inbox poll interval, and the
`CYRUP_SUBAGENT_*` env names the anchor register still writes and reads. Dropping them would have
turned "forward the ask to the session with a human on it" into a silent local-only fallback.
- **`cyrup-intercom` lost its subagent half (3,051 lines), not its feature.** Deleted: `seams.rs` (the
run-status/steer/terminate channel), `tools/contact_supervisor.rs`, `relay.rs`, the child fixture
binary, `ChildOrchestratorMetadata` + its reader + `preferred_supervisor_target` +
`format_child_orchestrator_message`, the native supervisor-channel probe, and
`ConnectParams::metadata`. Session identity claim/registration types the claimant crate used to
provide were vendored into `identity.rs`, and `cyrup_herdr::AGENT` ("cyrup") replaces the constant the
deleted crate exported for `name@machine` discovery. Broker, roster, `/intercom`, `/alias`,
`/handover`, pane labelling and cross-machine discovery are untouched. **intercom itself is kept** —
10 crates reference it and permission ask-forwarding rides its broker.
- **`cyrup-session-svc` lost the subagent-child tier.** `CYRUP_SUBAGENT_CHILD` gated which ambient
natives survived `--no-extensions`, because pi re-injects three extensions by path into a child so a
child never runs with its permission gate dropped. No process can be a child any more, so
`SUBAGENT_CHILD_ENV`, `SUBAGENT_CHILD_RUNTIME_NATIVES` and the `is_subagent_child` threading through
`native_survives_no_extensions` / `natives_to_load` are gone. SEAM-071's actual rule (PATH tier drops,
inline tier survives) is unchanged.
- **`cyrup-flux` no longer disables itself in a child** (`CYRUP_SUBAGENT_CHILD`), the guard's stated
reason being that a child would re-exec the binary per pipeline phase. Nested sessions were never
gated and still attach normally.
- **Accepted per D3/D4.** The herdr Agents-sidebar status feed and the seven `inspector.*` /
`project.*` tool verbs are gone — they were a module of the deleted crate and actions of the single
`subagent` tool. `cyrup-herdr` survives as the client intercom uses. Rebuilding the sidebar as a WASM
guest stays the optional follow-up below.
- **Tests.** `cyrup-it`'s `subagents` target (and its `build.rs` fixture entries) plus the
subagent-only `verify_redaction`, `child_bridge_activation`, `human_surface`, `shared_human_lock` and
`permission/forwarding_spawn_env` targets/modules are deleted; `intercom/broker_roundtrip` is kept and
simplified. The suite went 11 binaries → 9, ~862 → ~700 test functions. Kept and passing: `intercom`
170, `permission` 49, `bin` 90, `misc` 4, `session_svc` 60, `ext` 80 (the WASM host is unaffected,
per D6). Deleted with nothing to replace them: `cyrup-flux/tests/flux_002_subagent_fallback.rs` and
the two session-svc child-exemption tests.
- **Docs.** `docs/guide/extensions/subagents.md` deleted with its SUMMARY entry; the `subagents`
settings block, its two troubleshooting entries, README's feature/herdr/V8/workflowScript sections,
the crate table, the `cyrup-it` run recipe and the upstream-port table row are updated. The
intercom-bundled `pi-intercom` skill lost its "Pattern 7: Handle Subagent Escalations" section (the
ordering test in `resources.rs` now pins 6b against the Key Differences table). Prose in
`docs/gap-analysis/` — including the `09-cyrup-ext-subagents.md` ledger — is left as the historical
record it is rather than rewritten.

## Order

Planned `1 → 2 → 3 → 4`; taken **`1 → 4`** with step 2 absorbed (see above), step 3 still open.

Each step is its own branch off the previous, so any can be abandoned without losing the earlier ones.
Re-measure build size and time after step 2 — that is where codemode and V8 are gone, and it is the
natural point to decide whether step 4 is worth taking. In the event, V8 left at step 4: no
`deno_core`, no `libv8`, and the largest crate in the workspace is gone.

## Optional follow-up

Re-implement the herdr Agents sidebar reporter as a WASM guest extension, cyrup's analogue of herdr's
`herdr-agent-state.ts`. Needs no new WIT: the `exec` and `proc` interfaces exist, the herdr CLI already
exposes `report-agent` / `report-agent-session` / `report-metadata` / `release-agent`, and the gate is
env-only (`HERDR_ENV`, `HERDR_SOCKET_PATH`, `HERDR_PANE_ID`). `<agentDir>/extensions/` is a pre-trust
root, which is what `exec`/`proc` require. Guests get HTTP only, no raw sockets, so it must drive the
herdr CLI.
