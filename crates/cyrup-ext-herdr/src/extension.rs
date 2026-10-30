//! The built-in that owns the pane: it arms the bridge on `session_start`, releases it on
//! `session_shutdown`, and feeds the root turn's edges.
//!
//! # Why a built-in at all, rather than a hook in the session runtime
//!
//! The bridge needs three things, and all three already cross the extension seam: the session's
//! `session_start`/`session_shutdown` edges, its `agent_start`/`agent_end` edges, and the ONE
//! session-scoped [`cyrup_ext::host::HumanInteractionLock`] every companion that opens a human
//! prompt acquires
//! before opening it ([`HostServices::human_interaction_lock`]). Putting the bridge on the seam
//! means it needs no new host plumbing — and it is the seam that made the old placement invisible.
//! As `cyrup_ext_subagents::herdr` it was reached only from an extension that a default session
//! never attaches; as a built-in it is attached by the same loop that attaches `mcp` and
//! `permission-system`, and the only condition left is its own runtime gate
//! ([`crate::runtime`]'s four-state table).
//!
//! # What it does NOT own
//!
//! The fleet. A companion that has runs describes them through [`crate::fleet::FleetView`] and
//! raises its own attention rows; the subagents extension does that from its own `SessionStart`,
//! turn and run-ended edges. This built-in deliberately does not call
//! [`crate::HerdrBridge::sync_fleet`], and a companion must not call
//! [`crate::HerdrBridge::agent_start`]
//! or [`crate::HerdrBridge::agent_end`] — `edge_depth` is a refcount, and two feeders of the same
//! root turn would leave the pane at `working` until both balanced.

use std::sync::Arc;
use std::sync::{Mutex, PoisonError};

use cyrup_core::ExtensionId;
use cyrup_ext::{
    EventKind, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};

/// The id the extension loads under, and the name a user turns it off with in settings:
/// `{"extensions": ["-builtin:herdr-status"]}`. It is listed in `BUILTIN_EXTENSIONS`
/// (`crates/cyrup/src/session_launch.rs`) so it obeys that rule like the four built-ins pi names.
/// The other two switches are the env var below and `--no-extensions` (via
/// [`NativeExtension::is_ambient`]).
pub const EXTENSION_ID: &str = "herdr-status";

/// The operator's own switch: `CYRUP_HERDR_STATUS=0` (or `false`/`off`/`no`) attaches nothing.
///
/// Read ONCE, at attach (`crates/cyrup/src/session_launch.rs`), never per event — the per-event
/// cost of the whole feature outside a pane is one `Option` test, and re-reading the environment
/// on every turn would make that untrue. Unset is ON: this is a cyrup-only surface, and the
/// README's promise ("the herdr sidebar shows which pane is blocked waiting on you") is the
/// default behaviour, not an opt-in. `1`/`true`/`on`/`yes` are also ON, mirroring the companion
/// install vars (`CYRUP_INTERCOM`, `CYRUP_PERMISSION_SYSTEM`, `CYRUP_SUBAGENTS`); anything else
/// falls through to the default.
pub const STATUS_ENV_VAR: &str = "CYRUP_HERDR_STATUS";

/// Whether this launch attaches the built-in at all, off the process environment.
///
/// Off only on an explicit negative. An unparseable value is a typo to leave ON rather than a
/// reason to silently disable the feature the user is asking about — and a session that is not in a
/// herdr pane attaches it and does nothing anyway ([`crate::runtime`]'s first gate row is total
/// inertness), so a wrong-on is invisible while a wrong-off is the bug this crate exists to fix.
#[must_use]
pub fn attaches_by_env() -> bool {
    !matches!(
        std::env::var(STATUS_ENV_VAR).ok().as_deref().map(str::trim),
        Some("0") | Some("false") | Some("off") | Some("no")
    )
}

/// How long the `SessionShutdown` handler waits for the pane release.
///
/// **The release and the dispatcher's budget used to be the same five seconds.**
/// [`crate::shutdown`] is bounded by [`crate::reporter::RELEASE_TIMEOUT`] (5 s), this handler runs
/// under [`cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET`] (5 s), and the dispatcher enforces that
/// budget by DROPPING the handler future. A herdr that accepted the connection and then stopped
/// answering would therefore consume the whole budget on the arm's first statement, and on the
/// subagents extension — which used to own this teardown — every disposal after it was silently
/// dropped: nothing logged, nothing failed, the statements simply never ran.
///
/// So the release is SPAWNED, and only the join is bounded. A timed-out join does NOT abort it:
/// the task keeps draining on the runtime until `RELEASE_TIMEOUT` ends it, which on the `/quit`
/// path is inside `main`'s `runtime.dispose().await` (`crates/cyrup/src/main.rs:729-731`).
///
/// The `const` assertion below is the coupling: the join must leave at least as much of the
/// dispatcher's budget again for whatever runs after, so shrinking it is a compile error here
/// rather than a teardown that quietly stops happening.
const HERDR_RELEASE_JOIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

const _: () = assert!(
    HERDR_RELEASE_JOIN_BUDGET.as_millis() * 2
        <= cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET.as_millis(),
    "the release join must leave at least as much of the dispatcher's invoke budget as it takes"
);

/// The herdr pane status bridge, attached to every session that attaches built-ins.
#[derive(Default)]
pub struct HerdrStatusExtension {
    /// Bound by the session builder before `init` (`ExtensionHost::load_native_with_services`).
    /// `None` until then, and `None` forever for a caller that loads natives without a capability
    /// backend — which arms the bridge without the dialog watcher, exactly as
    /// [`crate::runtime::arm`] documents.
    services: Mutex<Option<Arc<dyn HostServices>>>,
    /// Named on the command line by `-e builtin:herdr-status`, which puts it in pi's
    /// `cliEnabledExtensions` tier — the one `--no-extensions` keeps. Mirrors
    /// `ToolSearchExtension::loaded_explicitly`.
    explicit: bool,
}

impl HerdrStatusExtension {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The extension as `-e builtin:herdr-status` loads it: an explicit path, which survives
    /// `--no-extensions` and so is not [ambient](NativeExtension::is_ambient).
    #[must_use]
    pub fn loaded_explicitly(mut self) -> Self {
        self.explicit = true;
        self
    }

    /// The one session-scoped human-wait slot, if a capability backend is bound.
    fn human_lock(&self) -> Option<Arc<cyrup_ext::host::HumanInteractionLock>> {
        self.services
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|services| services.human_interaction_lock())
    }
}

#[async_trait::async_trait]
impl NativeExtension for HerdrStatusExtension {
    fn id(&self) -> ExtensionId {
        ExtensionId::from(EXTENSION_ID)
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap_or_else(PoisonError::into_inner) = Some(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        // The four edges the bridge owns. Whether any of them is ACTED on is decided by the
        // runtime gate, not here: outside a her pane every delivery below is one `Option` test on
        // [`crate::bridge`], and nothing else. Nothing is registered — no tool, no command, no
        // flag — because this built-in contributes a side effect on the user's terminal, not a
        // surface in cyrup's.
        api.subscribe(&[
            EventKind::SessionStart,
            EventKind::SessionShutdown,
            EventKind::AgentStart,
            EventKind::AgentEnd,
        ]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { .. } => {
                // Idempotent (`crate::runtime::arm`), so a session switch inside one process is
                // safe, and the bridge that is already reporting keeps reporting rather than being
                // replaced mid-pane. The pane's fleet is fed by whichever companion has one — this
                // call is not it.
                crate::arm(ctx.has_ui, self.human_lock());
                HookOutcome::Noop
            }
            HostEvent::AgentStart => {
                if let Some(bridge) = crate::bridge() {
                    bridge.agent_start();
                }
                HookOutcome::Noop
            }
            HostEvent::AgentEnd { .. } => {
                if let Some(bridge) = crate::bridge() {
                    bridge.agent_end();
                }
                HookOutcome::Noop
            }
            HostEvent::SessionShutdown { .. } => {
                // STARTED first, JOINED last — see [`HERDR_RELEASE_JOIN_BUDGET`]. This is the
                // release that keeps a phantom `working` row out of the sidebar, and it is the
                // only statement in this arm that waits on something outside the process, so it is
                // the only one that can hang: spawning takes the global `BRIDGE` slot and starts
                // draining immediately, and the join is bounded by a slice of the dispatcher's.
                //
                // This is also the `kill` path, not only the `/quit` one: cyrup's own signal
                // handler (`crates/cyrup/src/signals.rs:317-330`) takes SIGTERM/SIGHUP and, on an
                // interactive host, fires the cancel token rather than exiting, so `main`'s
                // `runtime.dispose()` fans `session_shutdown{quit}` out here. The bridge therefore
                // installs NO signal handler of its own — a second one would exit the process out
                // from under this teardown.
                let release = tokio::spawn(crate::shutdown());
                match tokio::time::timeout(HERDR_RELEASE_JOIN_BUDGET, release).await {
                    Ok(Ok(())) => {}
                    Ok(Err(join)) => {
                        tracing::debug!(err = ?join, "herdr: the release task panicked");
                    }
                    Err(_) => {
                        // Not aborted: the task keeps draining until `RELEASE_TIMEOUT` ends it.
                        tracing::debug!(
                            budget_ms = HERDR_RELEASE_JOIN_BUDGET.as_millis(),
                            "herdr: the pane release had not landed within the join budget"
                        );
                    }
                }
                HookOutcome::Noop
            }
            _ => HookOutcome::Noop,
        }
    }

    /// Hidden from the startup `[Extensions]` listing, like every built-in (`resource-loader.ts:729`).
    fn is_hidden(&self) -> bool {
        true
    }

    /// Ambient, like the other three companions cyrup compiles in (permission-system, intercom,
    /// subagents): `--no-extensions` drops it. That is the flag a user reaches for when they want
    /// cyrup to touch nothing outside itself, and this built-in's entire output is outside itself —
    /// a row in someone else's terminal multiplexer.
    fn is_ambient(&self) -> bool {
        !self.explicit
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use cyrup_ext::{ExtMode, HostCtx};
    use std::path::PathBuf;

    fn ctx(has_ui: bool) -> HostCtx {
        HostCtx::event(ExtMode::Tui, has_ui, PathBuf::from("/tmp"))
    }

    /// The second row of the gate, observed from the built-in: a **headless** session passes the
    /// pane test at most and then drops — `arm` is handed `ctx.has_ui` verbatim, and `cyrup -p`
    /// never arms (the interactive parent owns the pane's lifecycle, and two cyrups reporting onto
    /// one pane would make the sidebar flicker between them). Env-independent on purpose: the test
    /// process may itself be inside a pane, so this asserts the half that does not depend on that.
    #[tokio::test]
    async fn a_headless_session_arms_nothing_and_every_later_edge_is_a_noop() {
        let ext = HerdrStatusExtension::new();
        ext.on_event(
            &HostEvent::SessionStart {
                reason: "startup".to_string(),
                previous_session_file: None,
            },
            &ctx(false),
        )
        .await;
        assert!(
            crate::bridge().is_none(),
            "headless: detected and dropped, nothing constructed"
        );
        ext.on_event(&HostEvent::AgentStart, &ctx(false)).await;
        ext.on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &ctx(false),
        )
        .await;
        // Safe with nothing armed: a shutdown that races a never-armed session is a no-op.
        ext.on_event(
            &HostEvent::SessionShutdown {
                reason: "quit".to_string(),
                target_session_file: None,
            },
            &ctx(false),
        )
        .await;
        assert!(crate::bridge().is_none());
    }

    /// The id a user turns this off with, pinned — settings name the built-in by this string.
    #[test]
    fn the_extension_id_is_the_settings_name() {
        assert_eq!(
            HerdrStatusExtension::new().id().as_str(),
            cyrup_core::ExtensionId::from(EXTENSION_ID).as_str()
        );
        assert_eq!(EXTENSION_ID, "herdr-status");
    }
}
