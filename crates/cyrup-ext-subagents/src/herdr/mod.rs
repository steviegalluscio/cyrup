//! The subagents companion's half of the herdr status bridge: what a fleet looks like to the
//! bridge, and the text it publishes.
//!
//! The bridge itself — gate, state machine, reporter task, event consumer, the built-in that arms
//! it — lives in [`cyrup_ext_herdr`], and it is attached whether or not this extension is. What is
//! here is the two things only this crate can supply:
//!
//! - [`fleet_view`] — the run-level rules: which runs count as work (pi's `Queued | Running`,
//!   de-duplicated across the in-memory and on-disk lists) and which are asking for the human.
//! - [`label`] — the presentation, and **the privacy rule**: raw prompts and child output never
//!   enter pane metadata.
//!
//! The direction matters. This crate feeds a [`cyrup_ext_herdr::fleet::FleetView`] to a bridge it
//! does not own; the bridge never reaches in here. That is what lets the feed exist in a session
//! where the subagents extension was never attached — the default, until
//! `CYRUP_SUBAGENTS` or a `subagents/config.json` opts in
//! (`crate::extension::host::registration`'s `gate_on_install`).
//!
//! # What this crate must NOT drive
//!
//! The root-turn edges. `agent_start`/`agent_end` are fed once, by `cyrup_ext_herdr`'s own
//! built-in, onto a saturating refcount; a second feeder of the same turn would leave the pane at
//! `working` until both balanced. What this crate feeds is levels ([`fleet_view`]), its own
//! foreground run edges (`foreground_run_started`/`foreground_run_finished`), and the attention
//! rows its runs raise.

pub mod label;

use cyrup_ext_herdr::{FleetView, RunAttention};

use crate::background::{ActivityState, RunId};
use crate::tui::fleet_state::{AsyncRunView, FleetState};

// Re-exported so the bridge is still reachable as `crate::herdr::*` from this crate and from its
// integration tests, after the move to `cyrup-ext-herdr`. `arm`/`arm_in` are here for the tests
// only — production arming is `cyrup_ext_herdr::extension`'s built-in, never this crate.
pub use cyrup_ext_herdr::{
    AGENT, AGENT_VIEW_ENV, AGENT_VIEW_LABEL, HerdrBridge, RunToken, SOURCE, arm, arm_in, bridge,
    reporter, runtime, shutdown, state,
};

pub use label::{
    MAX_TASK_LABEL_CHARS, MAX_TITLE_TASK_CHARS, MAX_WORKFLOW_LABEL_DEPTH, MAX_WORKFLOW_LABEL_NODES,
    RunLabel, bounded_task_label, metadata_text, workflow_task_label,
};

/// Every active detached background run in `fleet`, de-duplicated by run id.
///
/// `tracked_jobs` (in memory) and `history_jobs` (on disk, not tracked by this process) overlap —
/// pi filters the second against the first before concatenating (`fleet.ts:193,199`) — and a run
/// counted twice would keep the pane at `working` after it ended.
fn active_background_runs(fleet: &FleetState) -> Vec<&AsyncRunView> {
    let mut seen: Vec<&RunId> = Vec::new();
    let mut active: Vec<&AsyncRunView> = Vec::new();
    for run in fleet.tracked_jobs.iter().chain(fleet.history_jobs.iter()) {
        if !run.is_active() {
            continue;
        }
        if seen.iter().any(|id| **id == run.status.run_id) {
            continue;
        }
        seen.push(&run.status.run_id);
        active.push(run);
    }
    active
}

/// Turn this crate's fleet projection into the [`FleetView`] the bridge accepts.
///
/// **The one place the run-level rules are stated**, and the only function in this crate that
/// turns runs into published text ([`label::published_text`], one renderer, one producer). The
/// bridge's `HerdrBridge::sync_fleet` takes the result; it never sees an [`AsyncRunView`].
///
/// `project_panes` is the count of project panes this session has opened; `0` renders no pane
/// clause at all, exactly as pi does when its callback is absent (`herdr-status.ts:162-163`).
#[must_use]
pub fn fleet_view(fleet: &FleetState, project_panes: usize) -> FleetView {
    let active = active_background_runs(fleet);
    let attention: Vec<RunAttention> = active
        .iter()
        .map(|run| RunAttention {
            run_id: RunToken::new(run.status.run_id.to_string()),
            needs_attention: run.status.telemetry.activity_state
                == Some(ActivityState::NeedsAttention),
            message: None,
        })
        .collect();
    FleetView {
        active_runs: active.len(),
        attention,
        metadata: label::published_text(&active, project_panes),
    }
}

/// The run identity the bridge keys attention on, from this crate's [`RunId`].
///
/// The bridge must not name a subagents type, so the identity crosses as an opaque
/// [`RunToken`]; this is the one place the two are related.
#[must_use]
pub fn run_token(run_id: &RunId) -> RunToken {
    RunToken::new(run_id.to_string())
}

#[cfg(test)]
mod publish_tests;
