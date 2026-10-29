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

## Order

`1 → 2 → 3 → 4`

Each step is its own branch off the previous, so any can be abandoned without losing the earlier ones.
Re-measure build size and time after step 2 — that is where codemode and V8 are gone, and it is the
natural point to decide whether step 4 is worth taking.

## Optional follow-up

Re-implement the herdr Agents sidebar reporter as a WASM guest extension, cyrup's analogue of herdr's
`herdr-agent-state.ts`. Needs no new WIT: the `exec` and `proc` interfaces exist, the herdr CLI already
exposes `report-agent` / `report-agent-session` / `report-metadata` / `release-agent`, and the gate is
env-only (`HERDR_ENV`, `HERDR_SOCKET_PATH`, `HERDR_PANE_ID`). `<agentDir>/extensions/` is a pre-trust
root, which is what `exec`/`proc` require. Guests get HTTP only, no raw sockets, so it must drive the
herdr CLI.
