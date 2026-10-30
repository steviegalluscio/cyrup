//! The MCP surface of a launched session, over the real launch wiring and a real stdio server.
//!
//! `build_factory` attaches the MCP adapter and `tool_search` exactly as every mode does; the server
//! is a small `sh` script speaking MCP over stdio (the shape `cyrup-mcp`'s own live tests use), so
//! nothing about the connection, the discovery or the call is faked. The model is scripted: each of
//! its replies is the tool call the test names, and the answer the session gave that call is what is
//! asserted.
//!
//! **[CYRUP-DELTA]** pi drives this surface from inside `codemode` scripts (`tools.fx_echo(…)`);
//! this fork ships no codemode (docs/LEAN-FORK-PLAN.md), so every case below is driven by the model's
//! own tool call. It reaches the same adapter, the same registry, the same activation rules and the
//! same result conversion — only the caller differs.
//!
//! What each test pins, and the pi behaviour behind it (`extensions/mcp/index.ts`,
//! `extensions/mcp/tools.ts` and `core/agent-session.ts` @v1.0.4):
//!
//! * a call reaches a server that is still starting, then answers;
//! * every MCP tool resolves to the server's `CallToolResult` (`convertMcpResult`);
//! * `--tools` / `--exclude-tools` entries are patterns, and an allowlist that names no MCP tool
//!   leaves them registered (`_isAllowedTool`, `_isActivatable`);
//! * `--no-mcp` (`disabledBuiltinExtensions: ["mcp"]`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};

use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig};
use serde_json::{Value, json};

use super::tests::factory_with;

/// A stdio MCP server as an `sh` script. `$1` is the number of seconds it takes to start answering
/// (a first `npx` download). It serves four tools:
///
/// * `echo` answers `echoed:<text>`;
/// * `pic` answers a text block and an image block;
/// * `boom` answers `isError: true`;
/// * `data` answers `structuredContent`.
///
/// Free of `${`, `$env:` and `{env:`: the adapter interpolates `args`, and any of those would let
/// it rewrite the script.
const SERVER: &str = r#"
sleep "$1"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      pv=$(printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fx","version":"1"}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"echo back","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},{"name":"pic","description":"a picture","inputSchema":{"type":"object","properties":{}}},{"name":"boom","description":"fails","inputSchema":{"type":"object","properties":{}}},{"name":"data","description":"structured","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      case "$line" in
        *'"name":"pic"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"a picture"},{"type":"image","data":"iVBORw0KGgo=","mimeType":"image/png"}]}}\n' "$id"
          ;;
        *'"name":"boom"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"it broke"}],"isError":true}}\n' "$id"
          ;;
        *'"name":"data"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"rows"}],"structuredContent":{"rows":2}}}\n' "$id"
          ;;
        *)
          text=$(printf '%s' "$line" | sed -n 's/.*"text":"\([^"]*\)".*/\1/p')
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed:%s"}]}}\n' "$id" "$text"
          ;;
      esac
      ;;
    *'"method":"notifications/'*) : ;;
    *)
      if [ -n "$id" ]; then printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"; fi
      ;;
  esac
done
"#;

/// A tool call the scripted model makes: the tool name and its arguments.
type Call = (&'static str, Value);

/// What a run recorded: the tools each provider request declared, with their descriptions.
type Requests = Arc<Mutex<Vec<Vec<(String, String)>>>>;

/// A provider whose replies are `calls` in order and then a final text reply, recording what each
/// request declared and said.
fn scripted(requests: &Requests, calls: &[Call]) -> Arc<FauxProvider> {
    let steps: Vec<FauxResponseStep> = calls
        .iter()
        .map(|(name, args)| {
            let seen = Arc::clone(requests);
            let name = (*name).to_string();
            let args = args.clone();
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push(
                    ctx.tools
                        .iter()
                        .map(|tool| (tool.name.clone(), tool.description.clone()))
                        .collect(),
                );
                faux_assistant_message(
                    vec![faux_tool_call(name.clone(), args.clone())],
                    cyrup_core::StopReason::ToolUse,
                )
            })
        })
        .chain(std::iter::once({
            let seen = Arc::clone(requests);
            FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push(
                    ctx.tools
                        .iter()
                        .map(|tool| (tool.name.clone(), tool.description.clone()))
                        .collect(),
                );
                faux_assistant_message(vec![faux_text("done")], cyrup_core::StopReason::Stop)
            })
        }))
        .collect();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(steps);
    faux
}

/// One run of a launched session against the `fx` server.
struct Run {
    runtime: Arc<AgentSessionRuntime>,
    /// The tool names each request declared, in request order.
    declared: Vec<Vec<String>>,
    _tmp: tempfile::TempDir,
}

impl Run {
    /// The newest `tool` tool result: its text, whether it is an error result, and its details.
    async fn result(&self, tool: &str) -> (String, bool, Option<Value>) {
        self.runtime
            .session()
            .await
            .messages()
            .await
            .into_iter()
            .rev()
            .find_map(|message| match message {
                cyrup_core::Message::ToolResult {
                    tool_name,
                    content,
                    is_error,
                    details,
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
                    Some((text, is_error, details))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("a `{tool}` tool result"))
    }

    /// Every tool the session registered, active or not.
    async fn registered(&self) -> Vec<String> {
        self.runtime
            .session()
            .await
            .all_tools()
            .into_iter()
            .map(|row| row.name)
            .collect()
    }

    /// The `fx` server's tools the session registered, sorted. This is pi's `ALL_TOOLS` listing
    /// filtered to the server: what a model can be offered, and what a call can reach.
    async fn server_tools(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .registered()
            .await
            .into_iter()
            .filter(|name| name.starts_with("fx_"))
            .collect();
        names.sort();
        names
    }

    /// The tools of the LAST request that named a server tool — the request the model answered after
    /// the server connected.
    fn declared_server_tools(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .declared
            .last()
            .into_iter()
            .flatten()
            .filter(|name| name.starts_with("fx_"))
            .cloned()
            .collect();
        names.sort();
        names
    }
}

/// What a test chooses about the world: the server's entry beyond its command, `settings.json`, and
/// the seconds the server takes to start.
struct World {
    delay: u32,
    entry: Value,
    settings: Value,
}

impl World {
    fn eager(delay: u32) -> Self {
        Self {
            delay,
            entry: json!({ "directTools": true }),
            settings: json!({}),
        }
    }
}

/// Launch a session in `world` (flags applied by `configure`), let the model make `calls`, and return
/// what the session did.
async fn run(world: World, calls: &[Call], configure: impl FnOnce(&mut SessionConfig)) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut entry = json!({
        "command": "sh",
        "args": ["-c", SERVER, "sh", world.delay.to_string()],
        // A resident connection: a plain `lazy` server is discovered and then closed.
        "lifecycle": "keep-alive",
    });
    if let (Some(base), Some(extra)) = (entry.as_object_mut(), world.entry.as_object()) {
        base.extend(extra.clone());
    }
    std::fs::write(
        agent_dir.join("mcp.json"),
        json!({ "mcpServers": { "fx": entry } }).to_string(),
    )
    .unwrap();
    std::fs::write(agent_dir.join("settings.json"), world.settings.to_string()).unwrap();
    let requests: Requests = Arc::default();
    let provider = scripted(&requests, calls);
    let (factory, target) = factory_with(provider, &agent_dir, &cwd, false, configure);
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
    let session = runtime.session().await;
    let _ = session.prompt("call the server").await.unwrap();
    session.wait_for_idle().await;

    let declared = requests
        .lock()
        .unwrap()
        .clone()
        .into_iter()
        .map(|tools| tools.into_iter().map(|(name, _)| name).collect())
        .collect();
    Run {
        runtime,
        declared,
        _tmp: tmp,
    }
}

/// The four tools the fixture server serves.
const SERVER_TOOLS: [&str; 4] = ["fx_boom", "fx_data", "fx_echo", "fx_pic"];

/// A call to `echo` is answered by the server, even when the model made it before the server had a
/// chance to connect. The server takes a second to start, as a first `npx` download does, and the
/// session's own wait for the build is what makes the call answer rather than report a missing tool.
///
/// pi's case drove this from a script that subscribed to `tool_call` and waited for the server it
/// named; the wait under test here is the launch wiring's, which is the half that survives without
/// scripts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_call_waits_for_a_starting_mcp_server_and_the_server_answers_it() {
    let run = run(
        World::eager(1),
        &[("fx_echo", json!({ "text": "hi" }))],
        |_| {},
    )
    .await;
    let (text, is_error, _) = run.result("fx_echo").await;
    assert!(
        !is_error && text.contains("echoed:hi"),
        "the call did not reach the server: {text:?}"
    );
    let mut registered = run.server_tools().await;
    registered.sort();
    assert_eq!(
        registered, SERVER_TOOLS,
        "the server's tools are registered"
    );
}

/// Every MCP tool resolves to the server's `CallToolResult` for a direct call, eager ones included
/// (`convertMcpResult` + `outputSchema: createMcpResultSchema(...)` @v1.0.4): the image block is
/// there, `isError` becomes an error result carrying the server's text rather than a lost result,
/// and `structuredContent` comes through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_eager_mcp_tool_answers_with_the_servers_call_tool_result() {
    let run = run(
        World::eager(0),
        &[
            ("fx_echo", json!({ "text": "hi" })),
            ("fx_pic", json!({})),
            ("fx_boom", json!({})),
            ("fx_data", json!({})),
        ],
        |_| {},
    )
    .await;

    let (echo, is_error, _) = run.result("fx_echo").await;
    assert!(!is_error && echo.contains("echoed:hi"), "{echo:?}");

    let session = run.runtime.session().await;
    let pic_blocks = session
        .messages()
        .await
        .into_iter()
        .rev()
        .find_map(|message| match message {
            cyrup_core::Message::ToolResult {
                tool_name, content, ..
            } if tool_name == "fx_pic" => Some(
                content
                    .iter()
                    .map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => {
                            json!({ "type": "text", "text": text })
                        }
                        cyrup_core::Content::Image {
                            data, mime_type, ..
                        } => json!({
                            "type": "image",
                            "data": data,
                            "mimeType": mime_type,
                        }),
                        _ => json!({ "type": "other" }),
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .expect("the `fx_pic` result");
    assert_eq!(
        pic_blocks,
        vec![
            json!({ "type": "text", "text": "a picture" }),
            json!({
                "type": "image",
                "data": "iVBORw0KGgo=",
                "mimeType": "image/png"
            }),
        ],
        "the image block reaches the model"
    );

    let (boom, boom_is_error, _) = run.result("fx_boom").await;
    assert!(
        boom_is_error,
        "a server `isError` result is an error result"
    );
    assert!(boom.contains("it broke"), "{boom:?}");

    let (_, data_is_error, details) = run.result("fx_data").await;
    assert!(
        !data_is_error,
        "a `structuredContent` result is not an error"
    );
    assert!(
        details.is_some(),
        "the structured content is carried as the result's details"
    );
}

/// `--tools` entries are patterns, and an allowlist with no `mcp__` entry leaves MCP tools registered
/// but not declared (`_isAllowedTool`, `_isActivatable` @v1.0.4): a `directTools` (eager, `direct`)
/// tool the list matches is declared and callable, one it does not match is kept without being
/// active, and an inactive `direct` tool is not reachable by a call either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tools_pattern_activates_the_mcp_tools_it_matches() {
    let run = run(
        World::eager(0),
        &[("fx_echo", json!({ "text": "hi" }))],
        |cfg| cfg.tools = Some(vec!["read".into(), "fx_e*".into()]),
    )
    .await;

    let (echo, is_error, _) = run.result("fx_echo").await;
    assert!(!is_error && echo.contains("echoed:hi"), "{echo:?}");

    let declared = run.declared_server_tools();
    assert_eq!(
        declared,
        ["fx_echo".to_string()],
        "the pattern declares what it matches and nothing else"
    );
    let session = run.runtime.session().await;
    let active = session.active_tool_names();
    assert!(
        active.iter().any(|name| name == "fx_echo"),
        "the matched tool is active: {active:?}"
    );
    assert!(
        !active.iter().any(|name| name == "fx_pic"),
        "an unmatched eager tool is kept without being active: {active:?}"
    );
    assert!(
        run.server_tools().await.iter().any(|name| name == "fx_pic"),
        "the unmatched tool is still registered"
    );
}

/// A call to an eager MCP tool the allowlist kept but did not activate is refused — the registry
/// holds it, the active set does not, and that is what bounds the call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inactive_eager_mcp_tool_is_not_reachable_by_a_call() {
    let run = run(World::eager(0), &[("fx_pic", json!({}))], |cfg| {
        cfg.tools = Some(vec!["read".into(), "fx_e*".into()])
    })
    .await;
    let (text, is_error, _) = run.result("fx_pic").await;
    assert!(is_error, "the call to an inactive tool answered: {text:?}");
    assert!(!text.contains("a picture"), "{text:?}");
    assert!(
        run.server_tools().await.iter().any(|name| name == "fx_pic"),
        "the tool is registered, which is not the same as reachable"
    );
}

/// An allowlist that names no MCP tool leaves the server's tools in the REGISTRY but inactive, and
/// never declares them: the allowlist decides what the MODEL sees (`_isAllowedTool` vs
/// `_isActivatable` @v1.0.4 — a tool the list does not name is dropped only when the list speaks for
/// MCP tools at all, which an entry like `mcp__*` is).
///
/// **[CYRUP-DELTA]** pi additionally left such a tool callable from a `codemode` script without
/// activating it (`extensions/mcp/tools.ts` @v1.0.4). This fork ships no codemode, so the only
/// caller a registered-but-inactive tool has is `tool_search` (covered by the MCP-604 tests in
/// `session_launch::tests`), and the model's own call is refused — asserted here so the boundary is
/// pinned rather than merely undocumented.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_allowlist_without_an_mcp_entry_keeps_mcp_tools_registered_but_inactive() {
    let run = run(
        World::eager(0),
        &[("fx_echo", json!({ "text": "hi" }))],
        |cfg| cfg.tools = Some(vec!["read".into()]),
    )
    .await;
    assert_eq!(
        run.server_tools().await,
        SERVER_TOOLS,
        "an allowlist that says nothing about MCP tools dropped them"
    );
    for request in &run.declared {
        assert!(
            request.iter().all(|name| !name.starts_with("fx_")),
            "an MCP tool was declared under --tools read: {request:?}"
        );
    }
    let session = run.runtime.session().await;
    let active = session.active_tool_names();
    assert!(
        !active.iter().any(|name| name.starts_with("fx_")),
        "an MCP tool the allowlist does not name is active: {active:?}"
    );
    let (echo, is_error, _) = run.result("fx_echo").await;
    assert!(
        is_error && !echo.contains("echoed:hi"),
        "a model call reached an inactive tool the allowlist kept: {echo:?}"
    );
}

/// An allowlist entry for MCP tools (`mcp__*`) makes the list decide for them: the server's tools,
/// which it does not match, are dropped from the registry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_mcp_entry_in_the_allowlist_drops_the_mcp_tools_it_does_not_match() {
    let run = run(
        World::eager(0),
        &[("fx_echo", json!({ "text": "hi" }))],
        |cfg| cfg.tools = Some(vec!["read".into(), "mcp__other__*".into()]),
    )
    .await;
    assert_eq!(run.server_tools().await, Vec::<String>::new());
    let (text, is_error, _) = run.result("fx_echo").await;
    assert!(
        is_error && !text.contains("echoed:hi"),
        "a dropped tool answered anyway: {text:?}"
    );
}

/// `--exclude-tools` applies to MCP tools, patterns included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exclude_tools_patterns_remove_mcp_tools() {
    let run = run(
        World::eager(0),
        &[("fx_echo", json!({ "text": "hi" }))],
        |cfg| {
            cfg.exclude_tools = vec!["fx_p*".into(), "fx_boom".into()];
        },
    )
    .await;
    assert_eq!(
        run.server_tools().await,
        ["fx_data".to_string(), "fx_echo".to_string()]
    );
}

/// `--no-mcp` (`disabledBuiltinExtensions: ["mcp"]`, `main.ts` @v1.0.4): the built-in is not loaded,
/// no server connects, and no call reaches a server tool. The same launch without the flag is the
/// control: [`a_first_call_waits_for_a_starting_mcp_server_and_the_server_answers_it`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_mcp_loads_no_mcp_extension_and_no_server_connects() {
    let run = run(
        World::eager(0),
        &[("fx_echo", json!({ "text": "hi" }))],
        |cfg| {
            cfg.disabled_builtin_extensions
                .push(cyrup_mcp::EXTENSION_ID.to_string());
        },
    )
    .await;
    assert_eq!(run.server_tools().await, Vec::<String>::new());
    let session = run.runtime.session().await;
    let loaded: Vec<String> = session
        .services()
        .ext_host
        .loaded_ids()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        !loaded.iter().any(|id| id == cyrup_mcp::EXTENSION_ID),
        "the MCP extension is loaded under --no-mcp: {loaded:?}"
    );
    assert!(
        loaded.iter().any(|id| id == "tool-search"),
        "only MCP is off, not the other built-ins: {loaded:?}"
    );
    assert!(
        !session.all_tools().iter().any(|row| row.name == "mcp"),
        "the `mcp` gateway is registered"
    );
}
