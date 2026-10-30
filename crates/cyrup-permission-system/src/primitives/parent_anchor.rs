//! The process-wide parent-session anchor register (R-SA-P1, PERM-001) — vendored from the
//! deleted `cyrup-ext-subagents` crate (docs/LEAN-FORK-PLAN.md step 4), out of its
//! `background/parent_anchor.rs`, behaviour unchanged.
//!
//! [CYRUP-DELTA] With the subagents extension gone, this crate is the register's ONLY writer and
//! ONLY reader: the permission system publishes the live session id at `SessionStart` so a
//! forwarded ask from any child addresses the right spool, and clears it at session end. The
//! spawn-overlay half of the original module (`detached_runner_env_overlay*`) went with the crate —
//! nothing spawns a detached runner any more.

use std::sync::Mutex;

use crate::primitives::env::PARENT_SESSION_ENV_VAR;

/// The process-wide anchor slot — pi's `process.env[SUBAGENT_PARENT_SESSION_ENV]` cell
/// (`pi-subagents/src/extension/index.ts:716`), held as a `Mutex` because this crate forbids the
/// `unsafe` `std::env::set_var` a real env write would require. A plain `Mutex` (not a `OnceLock`)
/// for the same reason the executor's captured root anchor is one: pi's slot is CLEARABLE at
/// session shutdown (`:619`'s `delete`), which a write-once cell could not support.
static ROOT_PARENT_SESSION_ANCHOR: Mutex<Option<String>> = Mutex::new(None);

/// Publish this process's live session id as the parent-session anchor future spawns address
/// (pi `process.env[SUBAGENT_PARENT_SESSION_ENV] = sessionId`, `extension/index.ts:716`).
///
/// A blank id clears the slot rather than storing an empty string, so a headless / unpersisted
/// session never installs an anchor that would later resolve to a null forwarding target.
///
/// Call ONLY from a PARENT-role (root orchestrator) session start — see the module docs on why a
/// subagent child must not publish its own id.
pub fn publish_parent_session_anchor(session_id: &str) {
    let trimmed = session_id.trim();
    let mut slot = ROOT_PARENT_SESSION_ANCHOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *slot = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    };
}

/// Clear the published anchor (pi `delete process.env[SUBAGENT_PARENT_SESSION_ENV]`,
/// `extension/index.ts:765` @v0.43.0), so a stale id from a session that just ended never leaks into a
/// subsequently-started session on this same long-lived process.
pub fn clear_parent_session_anchor() {
    *ROOT_PARENT_SESSION_ANCHOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// The currently published anchor, if any — the register half of
/// [`resolve_parent_session_anchor`]'s ladder, exposed for assertions and diagnostics.
#[must_use]
pub fn published_parent_session_anchor() -> Option<String> {
    ROOT_PARENT_SESSION_ANCHOR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Resolve the anchor this process should hand to a child it is about to spawn: PUBLISHED (the
/// register above) → INHERITED (this process's own `CYRUP_SUBAGENT_PARENT_SESSION`) → none.
///
/// # Why PUBLISHED wins (PERM-001 follow-up)
///
/// pi has exactly ONE cell for this, `process.env[SUBAGENT_PARENT_SESSION_ENV]`, and publishing is
/// an **assignment** (`extension/index.ts:715` @v0.43.0): a session that publishes CLOBBERS whatever the process
/// inherited, and a spawn afterwards can only ever read the published value. cyrup cannot write
/// its own environment, so it emulates that one cell with TWO — the immutable inherited env plus
/// [`ROOT_PARENT_SESSION_ANCHOR`] — and the only faithful emulation of an assignment is for the
/// register to SHADOW the inherited value.
///
/// This ladder used to read INHERITED → PUBLISHED, justified in its own doc comment as "matching
/// `exec::build_attempt_spawn_plan`'s own ladder". It does not match it: that ladder is EXPLICIT
/// (`opts.parent_session_id`, sourced from `SubagentExecutor::root_parent_session` — i.e. the
/// captured/published anchor) → INHERITED (`exec/mod.rs`, pi `input.parentSessionId ??
/// process.env[…]`, `runs/shared/pi-args.ts:257`). Published-first is what makes the two agree, and what makes
/// the foreground and background paths agree with each other.
///
/// "But a nested orchestrator must keep threading the ROOT's anchor, not substitute its own" is
/// still true, and is still enforced — by the PUBLISHER, exactly where pi enforces it: a subagent
/// child never publishes. Upstream that is the `if (!process.env[SUBAGENT_CHILD_ENV])` guard at
/// `extension/index.ts:712` @v0.43.0; here it is `PermissionSystemExtension::publish_parent_session_anchor`'s
/// `install_watcher` gate, which is the SOLE writer of this register (a process carrying any of
/// `cyrup-permission-system`'s `SUBAGENT_ENV_HINT_KEYS` is built as `new_forwarding_child`, whose
/// `install_watcher` is `false` — the direct analog of pi's `SUBAGENT_CHILD_ENV` test). The hop-2
/// `__subagent-runner` process never publishes either, for a stronger reason: `main.rs`
/// pre-dispatches `__subagent-runner` before any session or extension is built, so no extension of
/// any kind exists in it — its [`resolve_parent_session_anchor`] call
/// (`background::runner_main`) reads an empty register and returns the inherited root anchor,
/// under either ordering.
///
/// (`SubagentsExtension`'s own `SessionStart` arm — likewise unreachable for a
/// `RegistrationMode::ChildSafe` child, which does not subscribe to it — calls
/// `SubagentExecutor::capture_parent_session_anchor`, a DIFFERENT cell: the executor's private
/// `root_parent_session` field feeding `RunOptions::parent_session_id` on the foreground path. It
/// does not write this register.)
///
/// So with no publisher in a child process the register is empty and the inherited root anchor is
/// returned either way — reordering cannot regress the nesting case. What it fixes is the case the
/// old order got wrong, and which pi's assignment makes impossible: a PARENT-role session in a
/// process that already carried `CYRUP_SUBAGENT_PARENT_SESSION` in its environment WITHOUT any
/// subagent hint (an SDK embedder, a test harness, or any launcher/CI that exported the variable —
/// no hint means `permission_extension_for_env` builds the PARENT role and publishes). Such a
/// session handed its detached runner a STALE ANCESTOR's id instead of its own live one, so every
/// forwarded ask from that background subtree addressed a spool with no watcher on it and
/// fail-closed denied.
///
/// Only a non-empty value is ever returned, so a caller can treat `Some` as "safe to write into a
/// child's environment".
#[must_use]
pub fn resolve_parent_session_anchor() -> Option<String> {
    resolve_parent_session_anchor_from(std::env::var(PARENT_SESSION_ENV_VAR).ok())
}

/// The injectable core of [`resolve_parent_session_anchor`], parameterized over the INHERITED env
/// value so the ladder is directly testable — this crate is `#![forbid(unsafe_code)]` and
/// `std::env::set_var` is `unsafe` as of the 2024 edition, so a test cannot install an inherited
/// value to assert against. Mirrors the same injectable-core convention
/// `spawn::resolve_spawn_command_from` / `spawn::depth::resolve_effective_depth_from` /
/// `background::spawn_detached::spawn_detached_runner_with_command` already use.
///
/// Both rungs are trimmed and empty-filtered before they can win, so neither an inherited `""`
/// (which a spawn overlay could legitimately carry) nor a whitespace-only publish is ever returned.
#[must_use]
pub fn resolve_parent_session_anchor_from(inherited: Option<String>) -> Option<String> {
    published_parent_session_anchor().or_else(|| {
        inherited
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
}
