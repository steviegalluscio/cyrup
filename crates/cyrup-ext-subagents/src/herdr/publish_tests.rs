//! The fleet→bridge seam: **the privacy rule on the production producer**, and the two run-level
//! rules the bridge is fed.
//!
//! These moved out of the bridge with the seam. They used to drive `HerdrBridge::sync_fleet` and
//! read back the reporter's decorative lane; now the production producer in this crate is
//! [`fleet_view`] — one function, one renderer ([`label::published_text`]) — and asserting on its
//! output pins the same mutations: reading a free-text run field, counting a run twice, dropping
//! the attention read, or rendering a pane clause that is not there.
//!
//! What the bridge does with a [`cyrup_ext_herdr::fleet::FleetView`] once it arrives is pinned in
//! `cyrup-ext-herdr`'s own `runtime` tests, not here.

use cyrup_ext_herdr::MetadataText;

use crate::background::{ActivityState, RunId};
use crate::herdr::fleet_view;
use crate::tui::fleet_state::{AsyncRunView, FleetState};

/// The agent every fixture run invokes. It is what a production workflow node's label degrades
/// to, so it is what the published summary must name.
const AGENT_NAME: &str = "reviewer";

/// The rendered strings of a published label, joined — what a human reading the pane sees.
fn published_strings(metadata: Option<MetadataText>) -> String {
    match metadata {
        Some(text) => format!(
            "{}\n{}",
            text.summary,
            text.title_suffix.unwrap_or_default()
        ),
        None => String::new(),
    }
}

/// An active detached run whose every free-text field carries `sentinel`, and whose workflow
/// graph is built by **the production builder**.
///
/// `workflow_graph_from_run` (`background/workflow_graph.rs:718`) is the only thing that
/// populates `status.telemetry.workflow_graph` in production — its one caller is
/// `background/runner_main/status.rs:44-46` — and it sets `label: None` and `phase: None` on
/// every `WorkflowTaskSpec` (`:752,:763,:775`), because `SingleStepSpec` carries neither. So a
/// published node label is the AGENT NAME or `Step N`, and a fixture that hand-built a node
/// with a rich `label` would be asserting over a shape production cannot emit. This one is
/// built from a `RunnerStep` list, so what it proves is what a user would see.
fn active_run(token: &str, needs_attention: bool, sentinel: &str) -> AsyncRunView {
    use crate::background::{RunMode, RunState, RunStatus, StepState, StepStatus};
    use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

    let run_id = RunId::from_token(token.to_string());
    let mut step = StepStatus::pending(AGENT_NAME);
    step.status = StepState::Running;
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Chain, None);
    status.steps = vec![step];
    status.state = RunState::Running;
    // The free-text field a careless label would reach for.
    status.error = Some(sentinel.to_string());
    if needs_attention {
        status.telemetry.activity_state = Some(ActivityState::NeedsAttention);
    }
    // The production graph, from the production builder. The step's `task` is the user's
    // prompt, and it is in here precisely so that a builder or a label that started reading it
    // would be caught.
    let steps = vec![RunnerStep::SingleStep(SingleStepSpec {
        worktree: crate::spawn::worktree::WorktreeRequest::Shared,
        machine: None,
        skills: None,
        session_dir: None,
        agent: AGENT_NAME.to_string(),
        task: sentinel.to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        fast: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
        label: None,
        session_name: None,
    })];
    status.telemetry.workflow_graph =
        Some(crate::background::workflow_graph_from_run(&steps, &status));
    AsyncRunView {
        paths: crate::background::RunPaths::for_run(
            std::path::Path::new("/tmp/async"),
            std::path::Path::new("/tmp/results"),
            &run_id,
        ),
        status,
        session_id: None,
        // pi's `AsyncJobState.description` — the free-text task the launch call supplied.
        description: Some(sentinel.to_string()),
        context: None,
        nested_children: Vec::new(),
    }
}

/// **The privacy rule, on the PRODUCTION producer.** `fleet_view` is what `native_impl.rs` calls
/// and `label::published_text` is the only function that turns runs into published text; this
/// drives the first and reads what the second put in its output.
///
/// The run here carries the sentinel in every free-text field a careless label would reach for:
/// `status.error` and `AsyncRunView::description` — the latter is pi's `AsyncJobState.description`,
/// the task string the launch call supplied, which [`label`]'s module doc lists under "what may
/// never be sent".
///
/// *Gutted by*: reading `AsyncRunView::description` in `RunLabel::from_async_run`; passing
/// `run.description.as_deref()` as `RunLabel::new`'s launch label inside `published_text`;
/// falling back to the description when the workflow graph yields nothing. Each of those is a
/// mutation of the ONE renderer, so each is RED here.
#[test]
fn raw_prompts_never_reach_the_published_label() {
    const SENTINEL: &str = "REFACTOR-THE-BILLING-SECRETS-PROMPT";
    let fleet = FleetState {
        tracked_jobs: vec![active_run("r1", false, SENTINEL)],
        ..FleetState::default()
    };
    let published = published_strings(fleet_view(&fleet, 0).metadata);
    assert!(
        !published.contains(SENTINEL),
        "the prompt reached pane metadata: {published}"
    );
    assert!(
        published.contains("reviewer"),
        "the agent name is what a production workflow graph can carry: {published}"
    );
}

/// The background level, and the de-duplication across the two lists. `tracked_jobs` (in memory)
/// and `history_jobs` (on disk) overlap — pi filters the second against the first before
/// concatenating (`fleet.ts:193,199`) — and a run counted twice would keep the pane at `working`
/// after it ended.
#[test]
fn a_resync_counts_each_active_run_once() {
    let fleet = FleetState {
        tracked_jobs: vec![active_run("r1", false, "x"), active_run("r2", false, "x")],
        // The same two runs, as the on-disk scan also sees them.
        history_jobs: vec![active_run("r1", false, "x"), active_run("r2", false, "x")],
        ..FleetState::default()
    };
    let view = fleet_view(&fleet, 0);
    assert_eq!(
        view.active_runs, 2,
        "two active detached runs, counted once each"
    );
    let published = published_strings(view.metadata.clone());
    assert!(
        published.contains("2 subagents"),
        "a run counted twice would read 4: {published}"
    );

    // Nothing active: the level drops to zero and the label is CLEARED rather than emptied — pi's
    // own path (`herdr-status.ts:203-215`), and the reason `metadata: None` is meaningful.
    let cleared = fleet_view(&FleetState::default(), 0);
    assert_eq!(cleared.active_runs, 0);
    assert_eq!(cleared.metadata, None);
}

/// A background run that has tripped the needs-attention heuristic raises an attention row — the
/// SECOND `blocked` source (the human-wait lock is the first and outranks it) — and the summary
/// gains the warning mark. A detached runner asking for the human is exactly what the sidebar has
/// to surface, because nothing else in the UI will.
///
/// Gutted by: dropping the `activity_state` read in `fleet_view`; dropping the attention count in
/// `published_text`.
#[test]
fn a_run_needing_attention_raises_and_marks_the_label() {
    let fleet = FleetState {
        tracked_jobs: vec![active_run("r1", true, "x")],
        ..FleetState::default()
    };
    let view = fleet_view(&fleet, 0);
    let raised: Vec<_> = view
        .attention
        .iter()
        .filter(|row| row.needs_attention)
        .collect();
    assert_eq!(
        raised.len(),
        1,
        "the run asks for the human, as a level the bridge can resync"
    );
    let published = published_strings(view.metadata);
    assert!(published.contains('⚠'), "{published}");
}

/// A run that is not asking for the human contributes no attention row — otherwise every pane
/// with a running child would read `blocked` and the sidebar would stop meaning anything.
#[test]
fn a_running_run_that_is_not_asking_raises_nothing() {
    let fleet = FleetState {
        tracked_jobs: vec![active_run("r1", false, "x")],
        ..FleetState::default()
    };
    let view = fleet_view(&fleet, 0);
    assert!(view.attention.iter().all(|row| !row.needs_attention));
    assert_eq!(view.active_runs, 1, "still work, so still `working`");
}

/// The project-pane clause renders only once that batch's manager exists. At `0` the clause is
/// absent entirely — pi does the same when its own callback is (`herdr-status.ts:162-163`).
#[test]
fn the_project_pane_clause_appears_only_when_there_are_panes() {
    let fleet = FleetState {
        tracked_jobs: vec![active_run("r1", false, "x")],
        ..FleetState::default()
    };
    assert!(!published_strings(fleet_view(&fleet, 0).metadata).contains("pane"));
    assert!(published_strings(fleet_view(&fleet, 2).metadata).contains("2 panes"));
}
