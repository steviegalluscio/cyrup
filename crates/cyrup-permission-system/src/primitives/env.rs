//! The `CYRUP_SUBAGENT_*` environment names this crate reads — vendored from the deleted
//! `cyrup-ext-subagents` crate (docs/LEAN-FORK-PLAN.md step 4), out of its `exec/spawn_plan.rs` and
//! `spawn/nested_events.rs`.
//!
//! These are a wire contract, not an implementation detail: a launching cyrup still puts them in a
//! child's environment (an SDK embedder, a shell, or any other launcher may do the same), and this
//! crate's role detection, permission persona lookup and ask-forwarding addressing all read them.
//! The names therefore stay exactly as pi names them upstream, `PI_SUBAGENT_*` → `CYRUP_SUBAGENT_*`.

/// Marks a process as a spawned child (pi `PI_SUBAGENT_CHILD`). This crate's
/// [`crate::extension::env::SUBAGENT_ENV_HINT_KEYS`] uses it to decide "this process is a child, so
/// it takes the forwarding role rather than the parent role".
pub const CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";

/// The child's own run id (pi `PI_SUBAGENT_RUN_ID`). A hint key: its presence says "I am a spawned
/// child", and it is what a nested child stamps on a forwarded ask.
pub const RUN_ID_ENV: &str = "CYRUP_SUBAGENT_RUN_ID";

/// The resolved persona name a launcher threads to a child so this crate's `agent` and
/// `projectAgent` policy layers enforce for the named persona (pi `resolveAgentName`,
/// `pi-permission-system/src/index.ts:2033-2047` @v0.7.1). Only a non-empty value is ever written.
pub const AGENT_NAME_ENV_VAR: &str = "CYRUP_SUBAGENT_AGENT_NAME";

/// The launching (parent) session's id — the routing key a child uses to address its parent's
/// ask-forwarding inbox (R-SA-P1). Published in-process by
/// [`crate::primitives::parent_anchor`] and inherited through the environment by spawned children.
pub const PARENT_SESSION_ENV_VAR: &str = "CYRUP_SUBAGENT_PARENT_SESSION";
