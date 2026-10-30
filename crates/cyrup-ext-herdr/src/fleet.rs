//! What a companion's work looks like **to the bridge** — the one type that crosses the seam.
//!
//! The bridge used to reach into its own crate's fleet projection
//! (`tui::fleet_state::{FleetState, AsyncRunView}`) and build the sidebar text itself. That is what
//! tied the whole herdr status feed to the subagents extension: the feed could not exist in a
//! session that had no subagents extension attached, and such a session is the default
//! (`extension/host/registration.rs`'s `gate_on_install`, default OFF). So the direction is
//! reversed here. The bridge owns the pane, the wire, the state machine and the sequence; a
//! companion that HAS a fleet describes it in this type and hands it over. With nothing to describe
//! the bridge still runs — the root turn and the human-wait lock are its own inputs — which is what
//! lets it arm in every herdr pane.
//!
//! Two halves, matching herdr's own split, and they stay separate on the wire:
//!
//! - [`FleetView::active_runs`] / [`FleetView::attention`] are **semantic**: they decide the
//!   `pane.report_agent` state (`working`, `blocked`) that drives herdr's waits, notifications and
//!   rollups.
//! - [`FleetView::metadata`] is **presentation**: it goes out through `pane.report_metadata` and
//!   nothing else keys on it.

use crate::state::RunAttention;

/// The two display-only strings the bridge publishes through `pane.report_metadata`.
///
/// Moved out of the subagents label layer with this type, because it is the payload, not the
/// recipe: whoever has runs renders them (see `cyrup-ext-subagents`'s `herdr::label`), and the
/// bridge only puts them on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataText {
    /// The `summary` token and the `idle`/`working`/`done` state labels — pi's `label(true)`
    /// (`herdr-status.ts:156-167`), which all three keys share (`:224-226`).
    pub summary: String,
    /// The `title-suffix` token — pi's `titleSuffix()` (`herdr-status.ts:169-176`).
    pub title_suffix: Option<String>,
}

/// A companion's fleet, as of one resync.
///
/// A **snapshot of levels**, not a delta: [`crate::state::StateModel`] treats the active count as a
/// level it recomputes, and rebuilds its attention set from [`FleetView::attention`] wholesale, so
/// a resync can only ever converge. A companion feeds one at session start (to pick up work that
/// outlived a previous process), at every run edge it sees, and on the bridge's own refresh tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetView {
    /// Runs in flight that the bridge should count as work — the level behind `working`.
    ///
    /// Whether a run counts is the companion's call, and it is NOT the same test for every kind:
    /// see `cyrup-ext-subagents`'s `herdr::fleet_view`, which keeps pi's `Queued | Running` rule
    /// and its de-duplication in one place.
    pub active_runs: usize,
    /// Which of those runs are asking for the human, in the order they asked.
    ///
    /// Insertion order is the input's to choose and the bridge preserves it: the reason it reports
    /// is the newest entry, matching pi's `[...attentionLabels.values()].at(-1)`
    /// (`herdr-status.ts:265`).
    pub attention: Vec<RunAttention>,
    /// The sidebar text for this snapshot, or `None` to clear the keys the bridge owns.
    ///
    /// `None` is pi's clear path (`herdr-status.ts:203-215`): with nothing running the bridge does
    /// not publish an empty label, it clears what it owns. Rendering `Some` is the companion's
    /// responsibility, including the privacy rule — raw prompts and child output must never reach
    /// a shared sidebar.
    pub metadata: Option<MetadataText>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// The default `FleetView` is "nothing is running", not "unknown": a session with no companion
    /// fleet arms with it and must not claim work, nor claim to be blocked.
    #[test]
    fn an_empty_view_claims_no_work_and_no_text() {
        let view = FleetView::default();
        assert_eq!(view.active_runs, 0);
        assert!(view.attention.is_empty());
        assert!(view.metadata.is_none());
    }
}
