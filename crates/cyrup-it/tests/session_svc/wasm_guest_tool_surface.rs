//! CODE-015, CODE-017 and the structured result of a guest tool — through a REAL session.
//!
//! The `cyrup-ext-sdk` demo component registers three tools for this surface: `structured_demo`
//! (declares an `outputSchema` and annotations, and returns `structuredContent`), `failing_demo`
//! (reports a failure without throwing) and `loadout_demo` (supplies a `prepareLoadout` hook).
//! Each test drives the component through `registration.register-tool`, the session's tool registry,
//! the provider request and a tool call the MODEL issues, so the WIT record, the host lift and the
//! `Tool` accessors are crossed, not hand-built.
//!
//! **[CYRUP-DELTA]** pi crosses this surface from inside a `codemode` script (`tools.structured_demo(…)`);
//! this fork ships no codemode (docs/LEAN-FORK-PLAN.md), so the model's own call is the caller. The
//! structured value a guest returns is read where a host reads it — the `tool_execution_end` event's
//! `result` — because the model itself never receives structured content (`pi`'s rule as well).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_provider::{Provider, ToolDef};
use cyrup_session_svc::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};
use futures::StreamExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::bins;

type Requests = Arc<Mutex<Vec<Vec<ToolDef>>>>;

/// A tool call the scripted model makes.
type Call = (&'static str, Value);

struct Rig {
    session: Arc<AgentSession>,
    faux: Arc<FauxProvider>,
    requests: Requests,
    /// Every `tool_execution_end` event: the tool's name, its `result`, and its error verdict.
    ends: Arc<Mutex<Vec<(String, Value, bool)>>>,
    _tmp: TempDir,
}

/// A session with the demo guest component and nothing else attached.
async fn rig() -> Rig {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let faux = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let session = SessionBuilder::new(faux.clone() as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap()
        .into_shared();
    session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bins::component_bytes(),
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load the demo guest");
    session.bind_extensions().await;

    let requests: Requests = Arc::default();
    let ends: Arc<Mutex<Vec<(String, Value, bool)>>> = Arc::default();
    let mut stream = session.subscribe();
    let sink = Arc::clone(&ends);
    tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            if let AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } = event
            {
                sink.lock().unwrap().push((tool_name, result, is_error));
            }
        }
    });

    Rig {
        session,
        faux,
        requests,
        ends,
        _tmp: tmp,
    }
}

impl Rig {
    async fn set_active(&self, names: &[&str]) {
        let names: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        self.session.set_active_tools_by_name(&names).await;
    }

    /// One prompt. When `calls` is given the model's replies are those tool calls, each followed by
    /// a final text reply; either way the provider records the tools of every request it receives.
    async fn prompt(&self, calls: &[Call]) {
        let steps: Vec<FauxResponseStep> = calls
            .iter()
            .map(|(name, args)| {
                let seen = Arc::clone(&self.requests);
                let name = (*name).to_string();
                let args = args.clone();
                FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                    seen.lock().unwrap().push(ctx.tools.clone());
                    faux_assistant_message(
                        vec![faux_tool_call(name.clone(), args.clone())],
                        StopReason::ToolUse,
                    )
                })
            })
            .chain(std::iter::once({
                let seen = Arc::clone(&self.requests);
                FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                    seen.lock().unwrap().push(ctx.tools.clone());
                    faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
                })
            }))
            .collect();
        self.faux.set_response_steps(steps);
        let _ = self.session.prompt("go").await.unwrap();
        self.session.wait_for_idle().await;

        // The end events are emitted on the fan-out; wait for the calls to have landed.
        let started = std::time::Instant::now();
        while self.ends.lock().unwrap().len() < calls.len()
            && started.elapsed() < std::time::Duration::from_secs(10)
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// The newest tool result the session recorded for `tool`: its text and whether it is an error.
    async fn tool_result(&self, tool: &str) -> (String, bool) {
        self.session
            .messages()
            .await
            .into_iter()
            .rev()
            .find_map(|message| match message {
                cyrup_core::Message::ToolResult {
                    tool_name,
                    content,
                    is_error,
                    ..
                } if tool_name == tool => {
                    let text = content
                        .iter()
                        .filter_map(|c| match c {
                            cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some((text, is_error))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("a `{tool}` tool result"))
    }

    /// The `tool_execution_end` event of `tool`, if one was seen.
    fn end_for(&self, tool: &str) -> Option<(Value, bool)> {
        self.ends
            .lock()
            .unwrap()
            .iter()
            .find(|(name, _, _)| name == tool)
            .map(|(_, result, is_error)| (result.clone(), *is_error))
    }

    fn first_request_tools(&self) -> Vec<ToolDef> {
        self.requests.lock().unwrap().first().cloned().unwrap()
    }
}

/// A guest tool that declares an `outputSchema` reaches the model as its TEXT, and its structured
/// value reaches the host as the `tool_execution_end` event's `structuredContent` — the WIT record
/// carries both across the boundary, and neither is the other (`ToolOutput::with_structured_content`,
/// pi `ToolResultEventResult.structuredContent`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_reaches_a_guest_tool_and_its_structured_content_reaches_the_host() {
    let rig = rig().await;
    rig.set_active(&["structured_demo"]).await;
    rig.prompt(&[("structured_demo", json!({ "n": 3 }))]).await;

    let (text, is_error) = rig.tool_result("structured_demo").await;
    assert!(!is_error, "the guest call succeeded: {text:?}");
    assert!(
        text.contains("doubled 3"),
        "the model gets the tool's text: {text:?}"
    );
    assert!(
        !text.contains("tags"),
        "the structured content is not folded into the text the model sees: {text:?}"
    );

    let (end, _) = rig
        .end_for("structured_demo")
        .expect("a tool_execution_end for the guest call");
    assert_eq!(
        end["structuredContent"],
        json!({ "doubled": 6, "tags": ["a", "b"] }),
        "the structured content the guest returned crossed the WIT boundary intact: {end}"
    );
}

/// A guest that reports a failure without throwing (`ToolOutput::error`) is an error result for the
/// call, with its text intact — the failure is a verdict, not an exception.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tool_that_reports_a_failure_is_an_error_result() {
    let rig = rig().await;
    rig.set_active(&["failing_demo"]).await;
    rig.prompt(&[("failing_demo", json!({}))]).await;

    let (text, is_error) = rig.tool_result("failing_demo").await;
    assert!(is_error, "the guest's failure is an error result: {text:?}");
    let (end, event_is_error) = rig
        .end_for("failing_demo")
        .expect("a tool_execution_end for the guest call");
    assert!(
        event_is_error,
        "the failure reached the event's verdict: {end}"
    );
}

/// `getAllTools` reports a guest tool's annotations (pi `ToolInfo.annotations`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tools_annotations_reach_the_session_tool_info() {
    let rig = rig().await;

    let tools = rig.session.all_tools();
    let info = tools
        .iter()
        .find(|t| t.name == "structured_demo")
        .expect("the guest tool is registered");
    let annotations = info.annotations.expect("annotations are reported");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.idempotent_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(false));
    assert_eq!(
        annotations.destructive_hint, None,
        "an unset hint stays unset"
    );
    let plain = tools.iter().find(|t| t.name == "demo_echo").unwrap();
    assert!(plain.annotations.is_none(), "a tool with none reports none");
}

/// CODE-015: a guest tool's `prepareLoadout` hook runs when the active tools are applied. The demo's
/// hook lists the callable tools in its own description and hides `signal_probe`'s declaration
/// while that tool is declared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_tools_prepare_loadout_hides_a_declaration_and_rewrites_a_description() {
    let rig = rig().await;
    rig.set_active(&["loadout_demo", "signal_probe", "demo_echo"])
        .await;

    rig.prompt(&[]).await;

    let sent = rig.first_request_tools();
    let names: Vec<&str> = sent.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"loadout_demo"), "{names:?}");
    assert!(names.contains(&"demo_echo"), "{names:?}");
    assert!(
        !names.contains(&"signal_probe"),
        "the guest's hook hid the declaration: {names:?}"
    );
    let description = sent
        .iter()
        .find(|t| t.name == "loadout_demo")
        .unwrap()
        .description
        .clone();
    assert!(
        description.starts_with("Orchestrates other tools. Callable: ")
            && description.contains("signal_probe")
            && description.contains("demo_echo"),
        "the guest's hook rewrote the description from the loadout it was handed: {description}"
    );
    // Hidden from requests only: the tool is still active and callable.
    assert!(
        rig.session
            .active_tool_names()
            .contains(&"signal_probe".to_string())
    );
}

/// The other direction: the hook shows a declaration again once nothing makes it hide, here
/// because the tool that hid it is no longer active.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_declaration_is_shown_again_when_the_hook_that_hid_it_is_not_active() {
    let rig = rig().await;
    rig.set_active(&["loadout_demo", "signal_probe"]).await;
    rig.set_active(&["signal_probe", "demo_echo"]).await;

    rig.prompt(&[]).await;

    let names: Vec<String> = rig
        .first_request_tools()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.contains(&"signal_probe".to_string()), "{names:?}");
}
