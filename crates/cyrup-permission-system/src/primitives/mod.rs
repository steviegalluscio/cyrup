//! Process/path primitives this crate used to borrow from `cyrup-ext-subagents`.
//!
//! [CYRUP-DELTA] docs/LEAN-FORK-PLAN.md step 4 deletes that crate (pi's subagents extension and
//! its background-execution machinery). `cyrup-permission-system` was its only remaining consumer
//! that survives the deletion — the ask-forwarding spool (`forwarding.rs`), the parent-session
//! anchor register (`extension/watcher.rs`, `extension/native.rs`) and the role/persona env reads
//! (`extension/env.rs`, `ask.rs`) — so the handful of primitives those layers need are vendored
//! here rather than kept alive by a crate whose whole purpose was spawning children.
//!
//! What is here is deliberately the narrow set, behaviour-identical to the originals, with their
//! pi citations intact:
//!
//! - [`env`] — the `CYRUP_SUBAGENT_*` names, a wire contract that outlives the spawner.
//! - [`parent_anchor`] — the process-wide parent-session anchor register (R-SA-P1, PERM-001).
//! - [`liveness`] / [`session_lease`] — the three-outcome liveness probe and the "demonstrably
//!   gone" ladder the `no-ui` marker is judged by.
//! - [`path_tokens`] — the R-SA-087 safe-token / containment guards.
//! - [`hostname`] — the same hostname the ownership stamps are written and compared with.

pub mod env;
pub mod hostname;
pub mod liveness;
pub mod parent_anchor;
pub mod path_tokens;
pub mod session_lease;

pub use env::{AGENT_NAME_ENV_VAR, CHILD_ENV, PARENT_SESSION_ENV_VAR, RUN_ID_ENV};
pub use hostname::machine_hostname;
pub use liveness::{
    Liveness, check_pid_identity_with, check_pid_liveness, check_pid_liveness_probing_zombie,
    current_pid_namespace_scope, pid_namespace_scope_differs,
};
pub use parent_anchor::{
    clear_parent_session_anchor, published_parent_session_anchor, publish_parent_session_anchor,
    resolve_parent_session_anchor, resolve_parent_session_anchor_from,
};
pub use path_tokens::{PathTokenError, validate_contains_root, validate_safe_token};
pub use session_lease::{
    OwnerStamp, ProcessStartIdentity, owner_demonstrably_gone, process_demonstrably_gone,
    process_start_identity, runtime_start_identity,
};

use std::time::Duration;

/// Target poll interval for the forwarding inbox watch (R-SA-082: "target ~250ms") — vendored from
/// the deleted crate's `background/control.rs`. The parent watcher's `notify::PollWatcher` and the
/// child's `wait_for_forwarded_approval` sleep both cadence on it.
pub const CONTROL_INBOX_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Milliseconds since the Unix epoch — the `runtime:` start-identity stamp and the forwarding
/// record timestamps. (The deleted crate kept this in its own `time` module.)
#[must_use]
pub fn now_epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
