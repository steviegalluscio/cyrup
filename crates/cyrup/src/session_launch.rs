//! Runtime assembly: build the session factory, attach the native built-ins, launch the runtime,
//! and apply the post-build session knobs.
//!
//! **Why this module exists.** The native-extension attachment sequence used to be written out
//! three times — once in each of the interactive, RPC and print/json arms of `main.rs` — and the
//! three copies were byte-identical for all 45 statements. Every new extension had to be added in
//! three places, and the explanatory comments had already drifted apart between the copies. The
//! runtime-creation + diagnostics-gate + post-build sequence was likewise duplicated between the
//! RPC and print/json arms (22 identical statements).
//!
//! The arms differ in exactly three ways, and each is now a parameter rather than a fork in the
//! source:
//!
//! * the interactive host supplies a `trust_prompt` and the other two do not — pi's `hasUI` gate
//!   (project-trust.ts:86-88), SEAM-065;
//! * the modelless hard stop is mode-gated — pi `main.ts:852-855` runs it for
//!   `appMode !== "interactive"` only, so interactive launches modelless and shows a banner
//!   instead (SEAM-075). That is [`PostBuild::require_model`];
//! * the interactive arm follows the launch with `time("resolveModelScope")` +
//!   `bind_extensions()`, which stay at the call site where their pi citations sit.
//!
//! Two apparent differences are NOT parameterised, because they are provably equivalent:
//! `.context(…)?` on the `create_unannounced` result is exactly the
//! `Err(anyhow::Error::new(e).context(…))` form the non-interactive arms spelled out, and the
//! unconditional [`crate::output_guard::restore_stdout`] is a no-op under interactive —
//! [`crate::cli::should_take_over_stdout`] is `mode != AppMode::Interactive && …`, so interactive
//! never installs the guard and `restore_stdout` is a bare `AtomicBool` store.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use cyrup_config::{AuthStore, ConfigDirs, ModelFile, SettingsManager, SettingsStore};
use cyrup_provider::Provider;
use cyrup_resources::package::manifest::BUILTIN_PATH_PREFIX;
use cyrup_session_svc::{
    AgentSession, AgentSessionRuntime, ScopedModel, SessionConfig, SessionFactory, SessionTarget,
    TrustPromptFn,
};

use crate::cli::Cli;
use crate::diagnostics::{self, Diagnostic};
use crate::timings;

/// Attach the native built-in extensions to `builder`, in pi's load order.
///
/// This is the ONE copy of a sequence that used to exist verbatim in all three mode arms. The
/// order is load-bearing:
///
/// 0. **llama.cpp** (EXT-027) goes FIRST, because pi loads its `builtInExtensions` ahead of
///    everything else: `extensionFactories = [...builtInExtensions, ...(options?.extensionFactories
///    ?? [])]` (`main.ts:569`) with `llama.cpp` the head of `builtInExtensions`
///    (`extensions/index.ts:7-8`). Attached in every mode and inside a subagent child, exactly as
///    upstream has it in every session — `llama_extension_for_env` never gates — unless the
///    `extensions` setting disables `builtin:llama.cpp` ([`BuiltinSelection`], EXT-094).
///    `--no-extensions` switches it off through `NativeExtension::is_ambient() -> true` (a child
///    is NOT among the natives its launcher re-injects, so it loses it too), and it stays out of
///    the startup `[Extensions]` list through `is_hidden` (`resource-loader.ts:729`,
///    `interactive-mode.ts:1778`) while remaining in `loaded_ids`.
/// 1. **Intercom** (port doc §7): the peer-session bus, attached when it is installed (the
///    `CYRUP_INTERCOM` flag or `<agent_dir>/intercom/config.json`).
/// 2. **The permission system** (port doc §4): the opt-in allow/ask/deny gate over tool calls.
/// 3. **Flux** (spec/flux.md §3.4.5): see item 6 below.
///
/// [CYRUP-DELTA] docs/LEAN-FORK-PLAN.md step 4 deletes pi's subagents extension, so the two
/// built-ins that existed only to serve it are gone from this list: the **SubAgents** extension
/// itself (with the intercom seam channels that had to be built before it, which is why intercom
/// used to be constructed `_concrete` and attached later) and the **subagent prompt runtime**
/// (SUBA-S01, pi `pi-args.ts:13`, which loaded `subagent-prompt-runtime.ts` into a spawned child as
/// its own extension and carried its `structured_output` / `bg_wait` tools and tool budget).
///
/// 4. **The MCP adapter** (gap-analysis 13a, MCP-001, the `pi-mcp-adapter` port). Attached
///    UNCONDITIONALLY — upstream is an installed npm package present in every session of every
///    mode, so there is no install gate to mirror; `--no-extensions` switches it off through
///    `NativeExtension::is_ambient() -> true`, which is the seam pi's own `noExtensions` uses on
///    the PATH tier that package lives in. It is NOT skipped inside a subagent child either: a
///    child resolves its `mcp:` tool selectors against the parent's `mcp-cache.json` and pins the
///    servers it needs with `MCP_DIRECT_TOOLS`. The `None` programmatic config is
///    `createMcpAdapter()`'s own default — discovery, not a caller-supplied config.
/// 5. **The permission system** (port doc §4): the opt-in allow/ask/deny gate over tool calls,
///    and returns `None` when the gate is not installed (DI-5) OR when an installed gate is
///    switched off by `"enabled": false` in its `config.json` (v0.8.0 `index.ts:1473-1477`, the
///    master switch that early-returns before registration). Its role selection used to read the
///    `CYRUP_SUBAGENT_CHILD` signal — a spawned child loaded the child→parent ask-FORWARDING channel
///    (P-4, forwarding.rs) and a root session the in-session dialog plus the forwarding watcher.
///    The forwarding watcher and the spool it watches stay; what is gone is the spawner, so in this
///    fork every cyrup process this gate builds is the root role unless an external launcher says
///    otherwise.
/// 6. **Flux** (spec/flux.md §3.4.5): the pipeline's bundled prompt templates + the three native
///    renderers. Attached unconditionally at the top level — unlike the three above there is no
///    install gate, because the whole point of moving flux into the binary is that it works with
///    no install step — the templates are EMBEDDED and materialised under `<agent_dir>/flux/`
///    on `ResourcesDiscover` (FLUX-001), which is why it takes `agent_dir` like the two above.
///    `flux_extension_for_env` used to return `None` inside a subagent CHILD (a marker only the
///    deleted subagents extension set), so a re-exec'd child
///    did not carry 15 templates plus a skill into a pipeline it was not running; there is no such
///    child any more.
///
/// The three natives pi ships as `builtin:<name>` extension resources — llama.cpp, tool-search
/// and mcp — attach only when `builtins` selects them (EXT-094): see [`BuiltinSelection`].
///
/// **[CYRUP-DELTA]** pi's fourth built-in, `codemode`, is not shipped by this fork (docs/LEAN-FORK-PLAN.md).
fn attach_native_extensions(
    mut builder: SessionFactory,
    dirs: &ConfigDirs,
    session_cwd: PathBuf,
    builtins: &BuiltinSelection,
) -> anyhow::Result<SessionFactory> {
    let agent_dir: &Path = &dirs.agent_dir;
    // `-e builtin:<name>` naming no built-in: pi's loader records `Unknown built-in extension:
    // builtin:<name>` against that path (`loadExtensionPaths`, `core/resource-loader.ts`). An
    // explicit `-e` path survives `--no-extensions`, so the placeholder is inline-tier.
    for name in &builtins.unknown {
        let path = format!("{BUILTIN_PATH_PREFIX}{name}");
        builder = quarantine(
            builder,
            &path,
            cyrup_ext::QuarantinedTier::Inline,
            &format!("Unknown built-in extension: {path}"),
        );
    }
    if builtins.attaches(cyrup_llama::LLAMA_PROVIDER_ID) {
        if builtins.is_explicit(cyrup_llama::LLAMA_PROVIDER_ID) {
            builder = builder.with_native_extension(Arc::new(
                cyrup_llama::LlamaExtension::new(agent_dir.to_path_buf()).loaded_explicitly(),
            ));
        } else if let Some(ext) = cyrup_llama::llama_extension_for_env(agent_dir) {
            builder = builder.with_native_extension(ext);
        }
    }
    // The bundled worked virtual-model router ([`crate::router_example`]), behind
    // `CYRUP_ROUTER_EXAMPLE`. Gated OFF by default and attached the same way `llama` is, so an
    // existing session's catalog is byte-for-byte unchanged unless the user asks for a router.
    // Without it the virtual-model feature has no in-tree registrant and is unreachable from the
    // shipped binary — pi ships its equivalent as `examples/extensions/jev-router.ts`, which cyrup
    // has no place for (natives are compile-time and there is no `examples/` directory).
    if let Some(ext) = crate::router_example::router_extension_for_env() {
        builder = builder.with_native_extension(ext);
    }
    // `tool_search` follows pi's second built-in (`extensions/index.ts:12` @v1.0.1): registered inactive and
    // replaceable, it loads `codemode` and `deferred` tools into the active set on request.
    if builtins.attaches(cyrup_tool_search::EXTENSION_ID) {
        let ext = cyrup_tool_search::ToolSearchExtension::new();
        builder = builder.with_native_extension(Arc::new(
            if builtins.is_explicit(cyrup_tool_search::EXTENSION_ID) {
                ext.loaded_explicitly()
            } else {
                ext
            },
        ));
    }
    // A malformed `intercom/config.json`, or an unusable `PI_INTERCOM_ASK_TIMEOUT_MS`, REFUSES
    // this extension. Both of upstream's equivalents throw from the first two lines of the
    // extension factory itself — `loadConfig()` (`pi-intercom/index.ts:648` @v0.16.0, throwing from
    // `config.ts:98-159`) and `getAskTimeoutMs()` (`:649`, throwing from `config.ts:18`) — so pi's
    // loader catches them, discards this one extension and builds the session without it. The
    // refusal therefore QUARANTINES the one extension; carrying it out of this function aborted the
    // whole launch instead, and not one of the other built-ins was attempted. See [`quarantine`].
    match cyrup_intercom::intercom_extension_for_env(agent_dir.to_path_buf(), session_cwd.clone()) {
        Ok(Some(ext)) => builder = builder.with_native_extension(ext),
        // Disabled by its own config, or simply not installed: attach nothing, as pi's absent
        // package does.
        Ok(None) => {}
        Err(refusal) => {
            builder = quarantine(
                builder,
                cyrup_intercom::EXTENSION_ID,
                // `IntercomExtension::is_ambient` is `true`: it stands in for an installed package
                // in the tier `--no-extensions` collapses, so the placeholder must be dropped by
                // that flag too.
                cyrup_ext::QuarantinedTier::Ambient,
                &refusal,
            );
        }
    }
    if builtins.attaches(cyrup_mcp::EXTENSION_ID) {
        if builtins.is_explicit(cyrup_mcp::EXTENSION_ID) {
            let mcp_dirs =
                cyrup_mcp::dirs::McpDirs::new(agent_dir.to_path_buf(), session_cwd.clone());
            builder = builder.with_native_extension(
                cyrup_mcp::McpExtension::new(mcp_dirs)
                    .loaded_explicitly()
                    .into_arc(),
            );
        } else if let Some(ext) =
            cyrup_mcp::mcp_extension_for_env(agent_dir, None, session_cwd.clone())
        {
            builder = builder.with_native_extension(ext);
        }
    }
    if let Some(ext) =
        cyrup_permission_system::permission_extension_for_env(agent_dir.to_path_buf(), session_cwd)
    {
        builder = builder.with_native_extension(ext);
    }
    if let Some(ext) = cyrup_flux::flux_extension_for_env(agent_dir) {
        builder = builder.with_native_extension(ext);
    }
    Ok(builder)
}

/// Replace the native built-in `id` with [`cyrup_ext::QuarantinedNative`] carrying `reason`, so a
/// built-in that REFUSED to be built costs this launch that one extension and nothing else.
///
/// # Upstream's blast radius, which this restores
///
/// pi runs every extension factory inside a `try`. A factory that throws is `load.discard()`ed,
/// the loader records `Failed to load extension: <message>` and the loop CONTINUES to the next
/// extension (`initializeExtension` + `loadExtension`'s catch,
/// `pi/packages/coding-agent/src/core/extensions/loader.ts:613-630`, `:655` @v1.0.1; the
/// inline tier has the same catch at `core/resource-loader.ts:1130-1141`). The session is built,
/// without that extension; `main.ts:914-922` then reports the recorded error and exits 1.
///
/// cyrup's one refusing built-in is intercom, whose `config.json` is malformed enough that
/// `load_config` names the path rather than degrading to defaults. It declines BEFORE the extension
/// object exists, so there is no factory for the loader to catch. Carrying the error out of this
/// function instead — which is what it did, along with the two subagents built-ins that
/// docs/LEAN-FORK-PLAN.md step 4 has since deleted (SUBA-166's refused `config.json`, CFG-080's
/// undecodable tool budget) — aborted the entire launch: no session, and no other built-in
/// attempted. That traded a fail-open for a strictly worse failure, since an ordinary typo in a
/// hand-edited `config.json` is not evidence of a tampered environment.
///
/// The placeholder is that missing throw. It registers nothing — `discard()` is therefore exact —
/// and fails `init`, which is the ONE path `cyrup-session-svc`'s build loop already treats as pi's
/// per-extension load failure (EXT-S01): the diagnostic lands on `StartupDiagnostics::extensions`
/// for the interactive `[Extension issues]` panel AND on `AgentSessionRuntime::diagnostics()` as a
/// fatal `Failed to load extension "<id>": <reason>`, which the bin reports and exits 1 on in
/// every mode. So the refusal still refuses — SUBA-166's config is still not loaded and CFG-080's
/// child still does not run — and it now refuses the way upstream does, through the same channel
/// every other extension-load failure already uses, with the other built-ins still attached.
///
/// The tests for this live with the quarantine cases below (`launch_with_intercom_config`).
fn quarantine(
    builder: SessionFactory,
    id: &str,
    tier: cyrup_ext::QuarantinedTier,
    reason: &dyn std::fmt::Display,
) -> SessionFactory {
    let reason = reason.to_string();
    tracing::error!(
        extension = %id,
        reason = %reason,
        "native built-in quarantined: it refused to be built"
    );
    builder.with_native_extension(Arc::new(cyrup_ext::QuarantinedNative::new(
        cyrup_core::ExtensionId::from(id),
        reason,
        tier,
    )) as Arc<dyn cyrup_ext::NativeExtension>)
}

/// Build the [`SessionFactory`] every mode launches from: the shared prefix plus
/// [`attach_native_extensions`].
///
/// `trust_prompt` is `Some` for the interactive host ONLY — SEAM-065. pi supplies its
/// `resolveProjectTrust` prompt callback only where it has a UI (`hasUI`, project-trust.ts:86-88);
/// every other host leaves it unset and the builder falls through to untrusted, exactly as pi's
/// `if (!hasUI) return false;` does. The trust STORE is not mode-gated and is wired for all three:
/// pi's `resolveProjectTrusted` reads it at project-trust.ts:72-75 for every host.
pub fn build_factory(
    provider: Arc<dyn Provider>,
    mut config: SessionConfig,
    settings_store: Arc<dyn SettingsStore>,
    auth_store: Arc<AuthStore>,
    dirs: &ConfigDirs,
    models_json: Arc<ModelFile>,
    trust_prompt: Option<TrustPromptFn>,
) -> anyhow::Result<Arc<SessionFactory>> {
    let session_cwd = config.cwd.clone();
    let builtins = BuiltinSelection::resolve(&mut config, &settings_store, dirs);
    // The resolver's providers read the SAME store `/login` writes through (`.auth` below).
    let credentials = cyrup_config::login::runtime_credentials(auth_store.clone());
    // SEAM-170 — every session this factory builds reads and refreshes THE process catalog
    // overlay slot, the one the background refresh, `cyrup update --models` and the provider
    // resolver below read. Without it each session got a private slot loaded at build that nothing
    // reloaded, so a model the background refresh found was never listed.
    let catalog =
        crate::provider::model_catalog_service(dirs, config.app_mode, config.model_network_enabled);
    let mut builder = SessionFactory::new(provider, config)
        .model_catalog_service(catalog)
        .settings_store(settings_store)
        .auth(auth_store)
        .trust_store(crate::prelaunch::trust_store_for(dirs));
    if let Some(prompt) = trust_prompt {
        builder = builder.trust_prompt(prompt);
    }
    builder = builder.provider_resolver(Arc::new(crate::provider::BuiltinProviderResolver::new(
        models_json,
        credentials,
    )));
    Ok(Arc::new(attach_native_extensions(
        builder,
        dirs,
        session_cwd,
        &builtins,
    )?))
}

/// The names pi's `builtInExtensions` gives its built-in extensions (`extensions/index.ts`), each
/// an extension resource `builtin:<name>`. cyrup's natives for them load under the same ids.
pub const BUILTIN_EXTENSIONS: [&str; 3] = [
    cyrup_llama::LLAMA_PROVIDER_ID,
    cyrup_tool_search::EXTENSION_ID,
    cyrup_mcp::EXTENSION_ID,
];

/// Which built-in extensions this launch attaches (EXT-094).
///
/// pi resolves each built-in as an extension resource (`DefaultPackageManager.resolve` and
/// `resolveExtensionSources`, `core/package-manager.ts`): it loads by default, a `-builtin:<name>`
/// (or `!` glob) in the `extensions` setting disables it, and `-e builtin:<name>` loads it
/// explicitly — into `cliEnabledExtensions`, which wins over the setting and which
/// `--no-extensions` keeps (`core/resource-loader.ts`). cyrup decides the first two here, at
/// attach; `--no-extensions` stays the builder's `is_ambient` gate, so an explicit built-in is
/// attached as a non-ambient one.
///
/// Only the USER `extensions` array is read, once per factory. pi lets a project `+`/`-`/`!` entry
/// override it and re-resolves on every `resolve()` (so `/reload` sees a changed setting), but
/// project settings exist only once project trust is resolved, and cyrup resolves that inside the
/// session build, after the natives are attached.
#[derive(Debug, Default)]
struct BuiltinSelection {
    /// Built-ins `-e builtin:<name>` named.
    explicit: Vec<String>,
    /// `-e builtin:<name>` names that are no built-in.
    unknown: Vec<String>,
    /// Built-ins the user `extensions` setting disables.
    disabled: Vec<&'static str>,
}

impl BuiltinSelection {
    /// Take the `builtin:` entries out of `config.extra_extension_paths` — they name no file for
    /// the discovery the builder runs over that list — and read the user `extensions` setting.
    fn resolve(
        config: &mut SessionConfig,
        settings_store: &Arc<dyn SettingsStore>,
        dirs: &ConfigDirs,
    ) -> Self {
        let mut selection = Self::default();
        config.extra_extension_paths.retain(|path| {
            let Some(name) = path
                .to_str()
                .and_then(|p| p.strip_prefix(BUILTIN_PATH_PREFIX))
            else {
                return true;
            };
            let bucket = if BUILTIN_EXTENSIONS.contains(&name) {
                &mut selection.explicit
            } else {
                &mut selection.unknown
            };
            if !bucket.iter().any(|n| n == name) {
                bucket.push(name.to_string());
            }
            false
        });
        let user = SettingsManager::load(Arc::clone(settings_store), false)
            .global()
            .extension_paths();
        selection.disabled = BUILTIN_EXTENSIONS
            .into_iter()
            .filter(|name| {
                !cyrup_resources::package::manifest::builtin_extension_enabled(
                    name,
                    &user,
                    &dirs.agent_dir,
                    &[],
                    &dirs.cwd,
                )
            })
            .collect();
        selection
    }

    /// Whether the built-in `name` is attached: named by `-e`, or not disabled by the setting.
    fn attaches(&self, name: &str) -> bool {
        self.is_explicit(name) || !self.disabled.contains(&name)
    }

    /// Whether `-e builtin:<name>` named it, which keeps it under `--no-extensions`.
    fn is_explicit(&self, name: &str) -> bool {
        self.explicit.iter().any(|n| n == name)
    }
}

/// The per-run, post-build session knobs, and whether pi's modelless hard stop applies to this
/// host. See [`apply_post_build`] for the knobs themselves.
pub struct PostBuild<'a> {
    /// The trimmed `--name` display name (pi `appendSessionInfo`, main.ts:586).
    pub session_name: Option<&'a str>,
    pub cli: &'a Cli,
    /// Whether this is a brand-new session (pi `!hasExistingSession`, main.ts:394).
    pub fresh: bool,
    /// pi `main.ts:852-855` — `if (appMode !== "interactive" && !session.model) { … exit(1) }`.
    /// `false` for the interactive host, which launches modelless and shows the
    /// `modelFallbackMessage` as a banner instead (SEAM-075).
    pub require_model: bool,
    /// The startup settings manager's diagnostics, merged with the runtime's at the checkpoint
    /// (pi `startupSettingsDiagnostics`, main.ts:657, :896). CFG-088.
    pub startup_diagnostics: &'a [crate::diagnostics::Diagnostic],
    /// Whether this is the interactive host: its merged diagnostics are handed to the TUI rather
    /// than printed (pi `appMode !== "interactive"`, main.ts:898).
    pub interactive: bool,
}

/// A runtime that passed [`launch`]'s checkpoints.
pub struct Launched {
    pub runtime: Arc<AgentSessionRuntime>,
    pub session: Arc<AgentSession>,
    /// The merged startup diagnostics an interactive run shows in its transcript (pi
    /// `InteractiveMode({ startupDiagnostics })`, main.ts:936); always empty for the other hosts,
    /// which printed them. CFG-088.
    pub notices: Vec<crate::diagnostics::Diagnostic>,
}

/// Create the runtime, run pi's post-creation diagnostics checkpoint, apply the post-build knobs,
/// and apply the mode-gated modelless stop.
///
/// [`ControlFlow::Break`] carries the exit code the caller must return; [`ControlFlow::Continue`]
/// carries the live runtime + session (and an interactive run's startup notices).
///
/// SEAM-033 — `create_unannounced` is pi's `createAgentSessionRuntime`
/// (agent-session-runtime.ts:414-432), which never emits `session_start`; the HOST announces.
/// `--name` (pi main.ts:650) and the scoped `--models` (pi main.ts:742-750) are applied in the
/// window this opens, so an extension's `session_start` handler observes the configured session
/// rather than an unnamed one on the pre-scope model. For print/json this also binds the
/// self-handle (via `into_shared`) so the post-run loop — auto-retry, post-run auto-compaction,
/// queued continuations — fires for one-shot runs.
///
/// AGENT-027 — pi's `main` timing sequence is ONE linear path covering every mode, so the three
/// runtime marks (main.ts:792/:798/:850) are taken here for all three arms.
pub async fn launch(
    factory: Arc<SessionFactory>,
    target: SessionTarget,
    post: PostBuild<'_>,
) -> anyhow::Result<ControlFlow<i32, Launched>> {
    timings::time("createRuntime", timings::TimingLabel::Main);
    let runtime = AgentSessionRuntime::create_unannounced(factory, target)
        .await
        .context("building agent session runtime")?;
    timings::time("createAgentSessionRuntime", timings::TimingLabel::Main);

    // `--help` (CFG-088 / SEAM-020) — pi's one help exit, `main.ts:857-864` @v0.87.1:
    //
    // ```ts
    // if (parsed.help) {
    //     reportDiagnostics(startupSettingsDiagnostics);
    //     const extensionFlags = resourceLoader.getExtensions()
    //         .extensions.flatMap((extension) => Array.from(extension.flags.values()));
    //     printHelp(extensionFlags);
    //     process.exit(0);
    // }
    // ```
    //
    // It sits HERE, straight after the runtime is built and BEFORE the diagnostics checkpoint
    // below (`:896-904`), because both halves of its output need the runtime and neither may wait
    // for the checkpoint: the flag list is the loaded extensions' declarations, and the startup
    // settings diagnostics are printed on their own — a runtime error (a conflicting extension
    // flag, say) does not stop `--help`. `process.exit(0)` runs no teardown, so neither does this:
    // `create_unannounced` never announced the session, and nothing is shut down.
    if post.cli.help {
        diagnostics::report(post.startup_diagnostics);
        let session = runtime.session().await;
        // `console.log` — through the stdout guard, which a `--mode json --help` run has
        // installed (pi's takeover is not lifted for `--help` either, `main.ts:639-642`).
        crate::output_guard::emit_stray(&crate::cli::render_help(
            &session.extension_flag_declarations(),
        ));
        return Ok(ControlFlow::Break(0));
    }

    // `--list-models [search]` (SEAM-135) — pi's second runtime-metadata exit, the sibling of the
    // `--help` one above and in the same place, `main.ts:866-871` @v0.87.1:
    //
    // ```ts
    // if (parsed.listModels !== undefined) {
    //     reportDiagnostics(startupSettingsDiagnostics);
    //     const searchPattern = typeof parsed.listModels === "string" ? parsed.listModels : undefined;
    //     await listModels(modelRuntime, searchPattern, AbortSignal.timeout(15_000));
    //     process.exit(0);
    // }
    // ```
    //
    // Running it on the built session (not before runtime creation, where cyrup used to exit) is
    // what lets the listing see a model an extension registered with `registerProvider`. Same
    // teardown story as `--help`: `process.exit(0)` runs none, and `create_unannounced` announced
    // nothing.
    if let Some(search) = post.cli.list_models.as_deref() {
        diagnostics::report(post.startup_diagnostics);
        let session = runtime.session().await;
        return Ok(ControlFlow::Break(crate::actions::list_models_for_session(
            &session, search,
        )));
    }

    // Pi main.ts:895-904 (SEAM-S01, CFG-088) — report the merged startup + runtime diagnostics
    // and exit 1 on any runtime error. Same checkpoint, every mode.
    let report =
        diagnostics::report_runtime(&runtime, post.startup_diagnostics, post.interactive).await;
    if report.fatal {
        runtime.dispose().await;
        crate::output_guard::restore_stdout();
        return Ok(ControlFlow::Break(1));
    }
    // pi's `time("createAgentSession")` sits directly after the same diagnostics gate
    // (main.ts:843-850).
    timings::time("createAgentSession", timings::TimingLabel::Main);
    let session = runtime.session().await;
    // SEAM-147 — pi resolves the scope and the CLI model BEFORE deciding `--api-key`
    // (`main.ts:812-838` @f1b2e77f5): the key needs `sessionOptions.model`, which only `--model` or
    // a fresh session's scope pick sets, and it goes to THAT model's provider. The `--model` /
    // `--provider` key is installed by `main.rs` before the build; the scope pick's is installed
    // here, before the pick is applied, because `set_model_resolved` runs pi's `hasConfiguredAuth`
    // precheck. pi raises the missing-model case as an error diagnostic, reported with the
    // runtime's and fatal (`:902-908`), so it sits here, after the `--help` / `--list-models` exits.
    let scope = resolve_launch_scope(&session, post.cli, post.fresh);
    if let Some(key) = post.cli.api_key.as_deref() {
        match api_key_target(post.cli, &scope) {
            ApiKeyTarget::FromCli => {}
            ApiKeyTarget::Scoped(provider) => {
                session
                    .services()
                    .auth
                    .set_runtime_api_key(provider, key.to_string());
            }
            ApiKeyTarget::Missing => {
                diagnostics::report(&[Diagnostic::error(API_KEY_REQUIRES_MODEL)]);
                runtime.dispose().await;
                crate::output_guard::restore_stdout();
                return Ok(ControlFlow::Break(1));
            }
        }
    }
    apply_post_build(&session, post.session_name, post.cli, scope).await;

    // Pi main.ts:852-855 — the modelless hard stop, gated on the MODE:
    //   `if (appMode !== "interactive" && !session.model) {`
    //   `    console.error(chalk.red(formatNoModelsAvailableMessage()));`
    //   `    process.exit(1);`
    //   `}`
    // It lives HERE, on the built session, not in the builder: `sdk.ts:216-218` resolves a
    // credential-less start to `model: undefined` + a `modelFallbackMessage` banner rather than an
    // error (SEAM-075), so every mode reaches this point and only the non-interactive ones stop.
    // Placed after `apply_post_build` because pi applies `--name` (main.ts:650) and the scoped
    // `--models` (main.ts:742-750) before :852.
    if post.require_model && session.model().is_none() {
        runtime.dispose().await;
        crate::output_guard::restore_stdout();
        diagnostics::no_models_available();
        return Ok(ControlFlow::Break(1));
    }
    Ok(ControlFlow::Continue(Launched {
        runtime,
        session,
        notices: report.notices,
    }))
}

/// pi's `--api-key` error text (`main.ts:833` @f1b2e77f5).
pub const API_KEY_REQUIRES_MODEL: &str =
    "--api-key requires a model to be specified via --model, --provider/--model, or --models";

/// The `--models` / `enabledModels` scope a launch resolved, and the model it picks for a fresh
/// session (pi `scopedModels` and `buildSessionOptions`' scope arm, `main.ts:812-815`, `:499-519`
/// @f1b2e77f5).
pub(crate) struct LaunchScope {
    scoped: Vec<ScopedModel>,
    /// pi `options.model` from the scope: set only with no `--model`, a non-empty scope and no
    /// existing session.
    pick: Option<ScopedModel>,
}

/// Resolve the launch scope, printing pi's scope warnings.
///
/// The patterns follow pi's precedence `parsed.models ?? settingsManager.getEnabledModels()`: an
/// explicit `--models` wins, even empty, otherwise the persisted `enabledModels` setting. They
/// resolve against [`AgentSession::available_model_catalog`] — pi's `resolveModelScope` runs over
/// `modelRuntime.getAvailable()` (`core/model-resolver.ts:364-370`), every provider with configured
/// auth — not against the installed provider's own catalog: SEAM-147 found that a `--models` naming
/// any provider other than the installed one (and, with the zero-model placeholder installed, any
/// provider at all) matched nothing. Matching itself is `cyrup-config`'s `minimatch`-faithful
/// resolver (see [`resolve_scoped_models_reporting`]).
///
/// `fresh` is pi's `!hasExistingSession`: a resumed session keeps its own restored model, so the
/// saved-default-in-scope pick fires only for a fresh one.
pub(crate) fn resolve_launch_scope(session: &AgentSession, cli: &Cli, fresh: bool) -> LaunchScope {
    let patterns = scope_patterns(cli.models.as_deref(), || {
        session.services().settings.effective().enabled_models()
    });
    if patterns.is_empty() {
        return LaunchScope {
            scoped: Vec::new(),
            pick: None,
        };
    }
    let catalog = session.available_model_catalog();
    // Pi `resolveModelScope` prints EVERY diagnostic its `WithDiagnostics` sibling collected —
    // `console.warn(chalk.yellow(`Warning: ${diagnostic.message}`))`, model-resolver.ts:378-380 —
    // before returning the (possibly empty) scope. Without this a typo'd
    // `--models "anthropc/*"` scoped nothing with no output at all.
    let (scoped, diags) = resolve_scoped_models_reporting(&catalog, &patterns);
    diagnostics::report(&diags);
    let pick = if cli.model.is_none() && fresh {
        let eff = session.services().settings.effective();
        pick_scoped_active_model(
            &scoped,
            eff.default_provider().as_deref(),
            eff.default_model().as_deref(),
        )
        .cloned()
    } else {
        None
    };
    LaunchScope { scoped, pick }
}

/// Where a launch's `--api-key` goes (pi `main.ts:830-838` @f1b2e77f5).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ApiKeyTarget {
    /// `--model` / `--provider` named the model; `main.rs` installed the key on its provider.
    FromCli,
    /// The scope picked the model; the key goes to its provider.
    Scoped(cyrup_sdk::core::ProviderId),
    /// No model was resolved: pi's error diagnostic, exit 1.
    Missing,
}

/// pi tests the RESOLVED `sessionOptions.model`, not which flags were typed: `--api-key k --models
/// ""`, a scope matching nothing, and a scope on a resumed session all leave it unset and error,
/// while an `enabledModels` scope picking a model on a fresh session satisfies it with no
/// `--models` at all. cyrup resolves a model from `--provider` alone, so `--provider` counts as
/// naming one (pi refuses a bare `--provider` outright, `main.ts:468-473`).
pub(crate) fn api_key_target(cli: &Cli, scope: &LaunchScope) -> ApiKeyTarget {
    if cli.model.is_some() || cli.provider.is_some() {
        return ApiKeyTarget::FromCli;
    }
    match &scope.pick {
        Some(pick) => ApiKeyTarget::Scoped(pick.model.provider.clone()),
        None => ApiKeyTarget::Missing,
    }
}

/// Apply the per-run, post-build session knobs that have no `SessionConfig` slot: the trimmed
/// `--name` display name (Pi `appendSessionInfo`, main.ts:586) and the resolved scope — the
/// fresh-session active-model pick (Pi `buildSessionOptions`, `main.ts:499-519` @f1b2e77f5) and the
/// Ctrl+P cycle set.
async fn apply_post_build(
    session: &AgentSession,
    name: Option<&str>,
    cli: &Cli,
    scope: LaunchScope,
) {
    if let Some(name) = name {
        let _ = session.set_session_name(name).await;
    }
    let LaunchScope { scoped, pick } = scope;
    if scoped.is_empty() {
        return;
    }
    if let Some(chosen) = pick
        && session.set_model_resolved(chosen.model).await.is_ok()
        // Use the scoped model's thinking level only when `--thinking` was omitted (explicit
        // `--thinking` takes precedence and is applied by the builder).
        && cli.thinking.is_none()
        && let Some(level) = chosen.thinking_level
    {
        let _ = session.set_thinking_level(level).await;
    }
    session.set_scoped_models(scoped);
}

/// Pi `modelPatterns = parsed.models ?? settingsManager.getEnabledModels()` (`main.ts:812-815`
/// @f1b2e77f5): a SUPPLIED `--models` wins even when it is empty (`--models ""` is pi's truthy
/// `[]`, which then skips scoping); the `enabledModels` setting is read only when the flag is
/// absent. SEAM-147.
fn scope_patterns(
    cli_models: Option<&[String]>,
    enabled_models: impl FnOnce() -> Option<Vec<String>>,
) -> Vec<String> {
    match cli_models {
        Some(patterns) => patterns.to_vec(),
        None => enabled_models().unwrap_or_default(),
    }
}

/// The saved-default-in-scope active-model pick (Pi `buildSessionOptions`, main.ts:394-414): given the
/// resolved `--models` scope and the settings default `(provider, model)`, prefer the saved default
/// when it is a member of the scope (case-insensitive `provider`+`id` match, Pi `modelsAreEqual`),
/// else the first scoped model. `None` only when the scope is empty.
fn pick_scoped_active_model<'a>(
    scoped: &'a [ScopedModel],
    saved_provider: Option<&str>,
    saved_model: Option<&str>,
) -> Option<&'a ScopedModel> {
    let saved = match (saved_provider, saved_model) {
        (Some(provider), Some(model)) if !provider.is_empty() && !model.is_empty() => {
            scoped.iter().find(|sm| {
                sm.model.provider.as_str().eq_ignore_ascii_case(provider)
                    && sm.model.id.as_str().eq_ignore_ascii_case(model)
            })
        }
        _ => None,
    };
    saved.or_else(|| scoped.first())
}

/// Pi `resolveModelScope(available, patterns)` in ONE call — both halves of its
/// `{ scopedModels, diagnostics }` return (model-resolver.ts:269-350 @v0.83.0).
///
/// CFG-008's residual. The diagnostics used to be REPLAYED in this binary: a per-pattern loop that
/// re-ran `resolve_scope` on a one-element slice to test emptiness, plus a hand-rolled recursion
/// that re-derived `parseModelPattern`'s colon-stripping to recover pi's `Invalid thinking level
/// "X" in pattern "Y". Using default instead.` sentence. Both of those now come from the resolver
/// that already computes them (`ModelScopeDiagnostic`, model.rs), so the glob/non-glob split, the
/// `:level` stripping and the exact-reference short-circuit exist in exactly one place.
///
/// Two notes the deleted replay carried, now settled rather than dropped:
///
/// * its `[CYRUP-DELTA]` about the missing `findExactModelReferenceMatch` short-circuit (:297-303)
///   was STALE — CFG-018 put the short-circuit into `resolve_scope_reporting`'s glob arm, so a
///   literal id containing `[` or `?` resolves instead of warning;
/// * its claim that `cyrup-config` "abbreviates the text to `invalid thinking level '<suffix>'`"
///   was also stale — `parse_pattern` mints pi's full sentence (model.rs, Pi `:243`).
///
/// The matching itself stays where it was: `cyrup-config`'s byte-for-byte
/// `minimatch({ nocase: true })` port (13,877-case verified) — a pattern containing `*`/`?`/`[` is
/// matched with real path-segment-aware globbing (`*` never crosses `/`, full `?`/`[...]`/`{a,b}`/
/// extglob support), and a non-glob pattern resolves to Pi's single best (alias-preferred,
/// `localeCompare`-tie-broken) model. This replaced the bespoke `*`-only, non-path-segment-aware
/// substring matcher that once lived in `cli.rs` (e.g. `anthropic*` wrongly matched every anthropic
/// model; `[...]` classes were unsupported). The resolver's `ScopedModel` is mapped onto the
/// session-svc `ScopedModel` here.
fn resolve_scoped_models_reporting(
    catalog: &[cyrup_provider::Model],
    patterns: &[String],
) -> (Vec<ScopedModel>, Vec<Diagnostic>) {
    let result = cyrup_config::ModelResolver::new(catalog).resolve_scope_reporting(patterns);
    let scoped = result
        .models
        .into_iter()
        .map(|sm| ScopedModel {
            model: sm.model,
            thinking_level: sm.thinking_level,
        })
        .collect();
    // Pi's rendering loop: `for (const diagnostic of diagnostics) console.warn(chalk.yellow(
    // `Warning: ${diagnostic.message}`))` (model-resolver.ts:355-361), reached on the live path at
    // main.ts:741-743 for both `--models` and the `enabledModels` fallback.
    let diagnostics = result
        .diagnostics
        .into_iter()
        .map(|d| Diagnostic::warning(d.message))
        .collect();
    (scoped, diagnostics)
}

/// The MCP surface of a launched session: the wait for a starting server, the result a tool call
/// receives, `--tools` patterns, `--no-mcp`, over a real stdio server.
#[cfg(test)]
#[path = "session_launch/mcp_tests.rs"]
mod mcp_tests;

/// A session replacement over the launch wiring: the extensions are rebound to the replacement's
/// backend, and the tool set the permission policy shapes is the same after it.
#[cfg(test)]
#[path = "session_launch/replacement_tests.rs"]
mod replacement_tests;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use crate::diagnostics::DiagnosticLevel;

    use super::{ScopedModel, pick_scoped_active_model, resolve_scoped_models_reporting};

    /// The models half of the one scope pass, for the tests that only assert matching.
    fn resolve_scoped_models_reporting_models(
        catalog: &[cyrup_provider::Model],
        patterns: &[String],
    ) -> Vec<ScopedModel> {
        resolve_scoped_models_reporting(catalog, patterns).0
    }

    /// The diagnostics half of the one scope pass.
    fn resolve_scoped_models_reporting_diagnostics(
        catalog: &[cyrup_provider::Model],
        patterns: &[String],
    ) -> Vec<super::Diagnostic> {
        resolve_scoped_models_reporting(catalog, patterns).1
    }

    /// The `--models`/`enabledModels` scope must report Pi's diagnostics, not resolve in silence
    /// (Pi `resolveModelScopeWithDiagnostics` → `resolveModelScope`, model-resolver.ts:270-361;
    /// live path main.ts:741-743). Before the fix `resolve_scope` returned only the matched set and
    /// `apply_post_build` dropped everything else on the floor, so a typo'd pattern was a silent
    /// no-op.
    #[test]
    fn scope_diagnostics_report_no_match_and_invalid_thinking_level_like_pi() {
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());

        // A pattern that matches nothing warns, in BOTH arms — the glob arm
        // (model-resolver.ts:311-318) and the non-glob arm (:334-341).
        for pattern in ["anthropc/*", "no-such-model-anywhere"] {
            let diags =
                resolve_scoped_models_reporting_diagnostics(&catalog, &[pattern.to_string()]);
            assert_eq!(diags.len(), 1, "{pattern}: {diags:?}");
            let only = diags.first().expect("one diagnostic");
            assert_eq!(only.level, DiagnosticLevel::Warning);
            assert_eq!(
                only.message,
                format!("No models match pattern \"{pattern}\"")
            );
        }

        // A pattern that DOES match is silent.
        assert!(
            resolve_scoped_models_reporting_diagnostics(&catalog, &["anthropic/*".to_string()])
                .is_empty(),
            "a matching pattern emits no diagnostic"
        );

        // An invalid `:level` suffix on a resolving pattern warns with Pi's exact sentence
        // (`parseModelPattern`, model-resolver.ts:243) and does NOT also warn no-match — the model
        // still resolves, at the default thinking level.
        let diags = resolve_scoped_models_reporting_diagnostics(
            &catalog,
            &["claude-opus-4-8:hihg".to_string()],
        );
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(
            diags.first().expect("one diagnostic").message,
            "Invalid thinking level \"hihg\" in pattern \"claude-opus-4-8:hihg\". Using default instead."
        );

        // A VALID `:level` is not a diagnostic at all.
        assert!(
            resolve_scoped_models_reporting_diagnostics(
                &catalog,
                &["claude-opus-4-8:high".to_string()]
            )
            .is_empty(),
            "a valid thinking level is silent"
        );

        // Both warnings can ride on one pattern list, in pattern order.
        let diags = resolve_scoped_models_reporting_diagnostics(
            &catalog,
            &["claude-opus-4-8:hihg".to_string(), "anthropc/*".to_string()],
        );
        assert_eq!(diags.len(), 2, "{diags:?}");
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(
            messages
                .first()
                .is_some_and(|m| m.starts_with("Invalid thinking level"))
        );
        assert!(
            messages
                .get(1)
                .is_some_and(|m| m.starts_with("No models match pattern"))
        );
    }

    /// CFG-008's residual. Pi's `resolveModelScope` returns `{ scopedModels, diagnostics }` from ONE
    /// pass (model-resolver.ts:269-350 @v0.83.0) and the live call site consumes both
    /// (main.ts:741-743). cyrup resolved twice — once for the models, once more per pattern to
    /// re-derive the warnings — so the diagnostic TYPE lived in this binary as a replay.
    ///
    /// This pins the pairing: for a list mixing a hit, a miss and a bad thinking level, ONE call
    /// yields the scoped set AND both warnings, in pattern order, and the two legacy entry points
    /// are the two halves of exactly that result.
    #[test]
    fn one_scope_pass_returns_pis_models_and_diagnostics_together() {
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());
        let patterns = vec![
            "anthropic/*".to_string(),
            "anthropc/*".to_string(),
            "claude-opus-4-8:hihg".to_string(),
        ];

        let (scoped, diagnostics) = resolve_scoped_models_reporting(&catalog, &patterns);

        // Presence before absence: the good pattern really did scope something.
        assert!(!scoped.is_empty(), "`anthropic/*` scopes a non-empty set");
        assert!(
            scoped
                .iter()
                .any(|s| s.model.id.as_str() == "claude-opus-4-8"),
            "the `:hihg` pattern still resolves its prefix — Pi keeps the model and drops the level"
        );
        assert!(
            scoped
                .iter()
                .filter(|s| s.model.id.as_str() == "claude-opus-4-8")
                .all(|s| s.thinking_level.is_none()),
            "an invalid level yields `thinkingLevel: undefined` (model-resolver.ts:237-245)"
        );

        let messages: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            vec![
                "No models match pattern \"anthropc/*\"",
                "Invalid thinking level \"hihg\" in pattern \"claude-opus-4-8:hihg\". Using default \
                 instead.",
            ],
            "diagnostics come back in pattern order, from the resolver"
        );
        assert!(
            diagnostics
                .iter()
                .all(|d| matches!(d.level, DiagnosticLevel::Warning)),
            "Pi renders every scope diagnostic through `console.warn` (model-resolver.ts:355-361)"
        );
    }

    /// SEAM-147 Verify — `--models "faux/*,"` resolves and warns exactly as `--models "faux/*"`,
    /// and `--models ""` still counts as a supplied flag and resolves no scope. pi `9b3c19da5`
    /// (`cli/args.ts:141-145` @v1.0.1) drops empty entries; `main.ts:812-815` @f1b2e77f5 reads
    /// `parsed.models ?? settingsManager.getEnabledModels()` and scopes only a non-empty list. The
    /// patterns go through the real clap parse and `normalize_list_flags`, then the same
    /// [`super::scope_patterns`] → [`resolve_scoped_models_reporting`] pair
    /// [`super::resolve_launch_scope`] runs. RED before the fix: the trailing comma added `No models match pattern ""`, and
    /// `--models ""` resolved the pattern `""` with that same warning. `anthropic/*` stands in for
    /// the Verify line's `faux/*`: `faux` is not in the bundled catalog this resolves against
    /// (`provider::tests::all_available_models_span_the_full_registry` asserts it absent), so
    /// `faux/*` would scope nothing on either side. What a supplied-but-empty scope does to the
    /// launch model and to `--api-key` is covered by `tests::api_key_models_scope`.
    #[test]
    fn a_trailing_comma_in_models_resolves_and_warns_like_the_bare_pattern() {
        use clap::Parser;
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());
        let patterns_for = |args: &[&str]| -> Vec<String> {
            let mut argv = vec!["cyrup"];
            argv.extend_from_slice(args);
            let mut cli = crate::cli::Cli::try_parse_from(argv).expect("parses");
            cli.normalize_list_flags();
            super::scope_patterns(cli.models.as_deref(), || Some(vec!["openai/*".to_string()]))
        };
        let messages = |patterns: &[String]| -> (Vec<String>, Vec<String>) {
            let (scoped, diags) = resolve_scoped_models_reporting(&catalog, patterns);
            (
                scoped
                    .iter()
                    .map(|s| format!("{}/{}", s.model.provider.as_str(), s.model.id.as_str()))
                    .collect(),
                diags.into_iter().map(|d| d.message).collect(),
            )
        };

        let bare = messages(&patterns_for(&["--models", "anthropic/*"]));
        assert!(!bare.0.is_empty(), "`anthropic/*` scopes something");
        assert!(bare.1.is_empty(), "{:?}", bare.1);
        assert_eq!(messages(&patterns_for(&["--models", "anthropic/*,"])), bare);
        assert_eq!(
            messages(&patterns_for(&["--models", " anthropic/* , ,"])),
            bare
        );

        // `--models ""`: supplied, so no `enabledModels` fallback, and nothing to scope or warn.
        let empty = patterns_for(&["--models", ""]);
        assert!(empty.is_empty(), "{empty:?}");
        assert_eq!(messages(&empty), (Vec::new(), Vec::new()));
        // Absent: the `enabledModels` setting is the scope source.
        assert_eq!(patterns_for(&[]), vec!["openai/*".to_string()]);
    }

    /// The live `--models`/`enabledModels` scope resolution must go through `cyrup-config`'s
    /// `minimatch`-faithful `ModelResolver::resolve_scope`, NOT the removed bespoke `*`-only matcher
    /// (gap-analysis 06 Gap #1; Pi `resolveModelScope`, model-resolver.ts:269-339). These are the
    /// exact divergences the crude matcher got wrong, verified against a real bundled catalog.
    #[test]
    fn resolve_scoped_models_uses_minimatch_semantics_like_pi() {
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());

        // Path-segment awareness: a 1-segment pattern (`anthropic*`, no `**`) can NEVER match the
        // 2-segment `anthropic/<id>` form under minimatch. The old crude matcher wrongly matched
        // EVERY anthropic model (its single segment anchored via plain substring across the `/`).
        //
        // It CAN match a bare id that genuinely begins "anthropic", and since `amazon-bedrock` was
        // ported that is no longer a hypothetical: Bedrock ids are dotted, e.g.
        // `anthropic.claude-opus-4-7`. So the assertion is no longer "zero matches" — it is that
        // every match is a BARE dotted id and none is the provider-qualified `anthropic/…` form.
        // Asserting emptiness here would have quietly re-encoded "amazon-bedrock is not ported".
        let scoped = resolve_scoped_models_reporting_models(&catalog, &["anthropic*".to_string()]);
        assert!(
            scoped.iter().all(|m| !m.model.id.as_str().contains('/')),
            "`anthropic*` is one segment, so it must never match the 2-segment `anthropic/<id>` \
             form; got {:?}",
            scoped
                .iter()
                .map(|m| m.model.id.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            scoped
                .iter()
                .all(|m| m.model.id.as_str().starts_with("anthropic")),
            "every match must actually begin with the literal pattern prefix; got {:?}",
            scoped
                .iter()
                .map(|m| m.model.id.as_str())
                .collect::<Vec<_>>()
        );

        // Character classes (`[68]`) are real minimatch syntax the crude matcher could not express
        // (it fell through to a literal-substring miss). Pi matches exactly the -6 and -8 opus ids.
        // (This used to read `[08]`; `claude-opus-4-0` was retired upstream in pi `cc2db980` — see
        // cyrup-provider `tests/catalog_data.rs`, PROV-004.)
        let scoped = resolve_scoped_models_reporting_models(
            &catalog,
            &["anthropic/claude-opus-4-[68]".to_string()],
        );
        let ids: Vec<&str> = scoped.iter().map(|s| s.model.id.as_str()).collect();
        assert!(
            ids.contains(&"claude-opus-4-6") && ids.contains(&"claude-opus-4-8"),
            "`anthropic/claude-opus-4-[68]` char-class must scope both opus ids, got {ids:?}"
        );
        assert!(
            scoped
                .iter()
                .all(|s| s.model.provider.as_str() == "anthropic"),
            "char-class stays path-segment-scoped to the anthropic provider"
        );

        // A bare `provider/*` glob is path-segment-aware: its first segment matches the whole
        // provider segment. Pi matches `minimatch(fullId) || minimatch(id)`, so every scoped model
        // is either an anthropic-provider model (fullId `anthropic/<id>`) or a model whose bare id
        // itself begins `anthropic/` (e.g. openrouter's `anthropic/claude-…`) — never an unrelated
        // provider like `anthropicX/…` (segment boundary, not a substring).
        let scoped = resolve_scoped_models_reporting_models(&catalog, &["anthropic/*".to_string()]);
        assert!(!scoped.is_empty(), "`anthropic/*` scopes a non-empty set");
        assert!(
            scoped
                .iter()
                .any(|s| s.model.provider.as_str() == "anthropic"),
            "`anthropic/*` includes the anthropic provider's own models"
        );
        assert!(
            scoped
                .iter()
                .all(|s| s.model.provider.as_str() == "anthropic"
                    || s.model.id.as_str().starts_with("anthropic/")),
            "every `anthropic/*` match is anthropic-provider or an `anthropic/`-prefixed id (Pi's \
             `minimatch(fullId) || minimatch(id)`)"
        );
    }

    fn scoped(provider: &str, id: &str) -> ScopedModel {
        // Build a `ScopedModel` from a real catalog entry so the pick exercises real `Model` fields.
        let catalog = crate::provider::all_available_models(&cyrup_config::ModelFile::default());
        let model = catalog
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == id)
            .or_else(|| catalog.iter().find(|m| m.provider.as_str() == provider))
            .expect("a catalog model for the provider")
            .clone();
        ScopedModel {
            model,
            thinking_level: None,
        }
    }

    // ============================================================================================
    // EXT-027 — the llama.cpp built-in is attached by `attach_native_extensions`.
    //
    // Every test drives the production seam, `build_factory` (the one caller of
    // `attach_native_extensions`), then builds a REAL session off the factory and reads what the
    // extension host actually loaded. Nothing here constructs the extension by hand, so removing the
    // `with_native_extension` line in `attach_native_extensions` fails every one of them.
    // ============================================================================================

    use std::path::Path;
    use std::sync::Arc;

    use cyrup_provider::Provider;
    use cyrup_provider::faux::FauxProvider;
    use cyrup_session_svc::{
        AgentSession, AgentSessionRuntime, RuntimeDiagnostic, SessionConfig, SessionFactory,
        TrustPromptFn,
    };

    use super::{BUILTIN_EXTENSIONS, build_factory};

    /// The id both [`cyrup_llama::LlamaExtension`] and the provider it registers carry
    /// (`provider.ts` `LLAMA_PROVIDER_ID`).
    const LLAMA_ID: &str = cyrup_llama::LLAMA_PROVIDER_ID;

    /// The always-attached sibling that follows llama.cpp in pi's order (`extensions/index.ts:7-12`
    /// lists llama.cpp before `mcp`); cyrup's MCP adapter is `ambient` like it.
    const MCP_ID: &str = cyrup_mcp::EXTENSION_ID;

    /// A trust prompt that is never consulted (`trust_override` answers first) — it exists so the
    /// interactive arm's `Some(prompt)` shape is the one under test (pi's `hasUI` gate,
    /// project-trust.ts:86-88).
    fn never_asked_trust_prompt() -> TrustPromptFn {
        Arc::new(|_, _| Box::pin(async { Some(true) }))
    }

    /// What a session built through `build_factory` reports: every id the host loaded (in load
    /// order) and the ids the startup `[Extensions]` panel lists.
    struct Loaded {
        loaded: Vec<String>,
        listed: Vec<String>,
    }

    /// The factory a mode arm would build over `agent_dir` + `cwd`, and the target to build it at.
    /// Shared by [`session_through_build_factory`] (which is handed a throwaway home) and by the
    /// quarantine tests, which have to PREPARE that home — write an `intercom/config.json` into it
    /// — before the factory is built.
    fn factory_at(
        agent_dir: &Path,
        cwd: &Path,
        interactive: bool,
        no_extensions: bool,
    ) -> (Arc<SessionFactory>, cyrup_session_svc::SessionTarget) {
        factory_over(
            Arc::new(FauxProvider::new()),
            agent_dir,
            cwd,
            interactive,
            no_extensions,
        )
    }

    /// [`factory_at`] over a provider the test scripts.
    fn factory_over(
        provider: Arc<dyn Provider>,
        agent_dir: &Path,
        cwd: &Path,
        interactive: bool,
        no_extensions: bool,
    ) -> (Arc<SessionFactory>, cyrup_session_svc::SessionTarget) {
        factory_with(provider, agent_dir, cwd, interactive, |config| {
            config.no_extensions = no_extensions;
        })
    }

    /// [`factory_over`] with the per-run [`SessionConfig`] knobs set by `configure` (`--tools`,
    /// `--exclude-tools`, ...).
    pub(super) fn factory_with(
        provider: Arc<dyn Provider>,
        agent_dir: &Path,
        cwd: &Path,
        interactive: bool,
        configure: impl FnOnce(&mut SessionConfig),
    ) -> (Arc<SessionFactory>, cyrup_session_svc::SessionTarget) {
        let env = cyrup_config::EnvVars {
            home: Some(agent_dir.to_path_buf()),
            ..cyrup_config::EnvVars::default()
        };
        let overrides = cyrup_config::CliConfigOverrides {
            agent_dir: Some(agent_dir.to_path_buf()),
            cwd: Some(cwd.to_path_buf()),
            ..Default::default()
        };
        let dirs = cyrup_config::ConfigDirs::resolve(&overrides, &env).unwrap();

        let mut config = SessionConfig::new(cwd.to_path_buf(), agent_dir.to_path_buf());
        config.persist = false;
        config.trust_override = Some(true);
        configure(&mut config);
        let target = config.target.clone();

        let factory = build_factory(
            provider,
            config,
            crate::file_settings_store(&dirs),
            Arc::new(cyrup_config::AuthStore::at(agent_dir.join("auth.json"))),
            &dirs,
            Arc::new(cyrup_config::ModelFile::default()),
            interactive.then(never_asked_trust_prompt),
        )
        .unwrap();
        (factory, target)
    }

    /// Build a session exactly as a mode arm does — through `build_factory` — over the faux
    /// provider in a hermetic temp home. `interactive` selects the one thing the three arms differ
    /// by in this function: whether a `trust_prompt` is supplied.
    async fn session_through_build_factory(interactive: bool, no_extensions: bool) -> Loaded {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let (factory, target) = factory_at(&agent_dir, &cwd, interactive, no_extensions);
        let session: AgentSession = factory.build(target, None).await.unwrap();

        let loaded = session
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        let listed = cyrup_tui::StartupReport::from_session(&session, false).extensions;
        Loaded { loaded, listed }
    }

    /// [`session_through_build_factory`] (no `--no-extensions`) with `extensions` passed as `-e`.
    async fn session_with_extensions(interactive: bool, extensions: &[String]) -> Loaded {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let (factory, target) = factory_with(
            Arc::new(FauxProvider::new()),
            &agent_dir,
            &cwd,
            interactive,
            |config| {
                config.extra_extension_paths =
                    extensions.iter().map(std::path::PathBuf::from).collect();
            },
        );
        let session: AgentSession = factory.build(target, None).await.unwrap();
        let loaded = session
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        let listed = cyrup_tui::StartupReport::from_session(&session, false).extensions;
        Loaded { loaded, listed }
    }

    /// The position of `id` in `ids`, failing loudly (with the whole list) when absent.
    fn position(ids: &[String], id: &str) -> usize {
        ids.iter()
            .position(|i| i == id)
            .unwrap_or_else(|| panic!("{id} is not loaded; got {ids:?}"))
    }

    /// Attached in all three modes `attach_native_extensions` serves — interactive (a trust prompt
    /// is supplied) and RPC / print / json (it is not) — and FIRST in the order, ahead of the
    /// always-attached MCP adapter (pi `main.ts:569`: `[...builtInExtensions, ...user factories]`,
    /// `extensions/index.ts:7-12`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn llama_is_attached_first_in_every_mode() {
        for (mode, interactive) in [("interactive", true), ("rpc/print/json", false)] {
            let Loaded { loaded, .. } = session_through_build_factory(interactive, false).await;
            assert_eq!(
                loaded.first().map(String::as_str),
                Some(LLAMA_ID),
                "{mode}: llama.cpp is loaded before every other native (pi main.ts:569); got {loaded:?}"
            );
            assert!(
                position(&loaded, LLAMA_ID) < position(&loaded, MCP_ID),
                "{mode}: llama.cpp precedes the MCP adapter (extensions/index.ts:7-12); got {loaded:?}"
            );
        }
    }

    /// Hidden at startup, loaded regardless: pi lists `extensions.filter((e) => !e.hidden)`
    /// (`interactive-mode.ts:1778`) and marks every `builtin:` extension hidden
    /// (`resource-loader.ts:741` @f1b2e77f5). `cyrup-flux`, which is no pi built-in, in the same
    /// breath proves the list is not simply empty.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn llama_is_loaded_but_absent_from_the_startup_extensions_list() {
        let Loaded { loaded, listed, .. } = session_through_build_factory(true, false).await;
        assert!(
            loaded.iter().any(|i| i == LLAMA_ID),
            "llama.cpp is in loaded_ids; got {loaded:?}"
        );
        assert!(
            !listed.iter().any(|i| i == LLAMA_ID),
            "llama.cpp is hidden from the startup [Extensions] list; got {listed:?}"
        );
        assert!(
            listed.iter().any(|i| i == "cyrup-flux"),
            "a non-hidden native IS listed, so the list is not trivially empty; got {listed:?}"
        );
    }

    /// EXT-092 — hidden follows the load path, as pi infers it: a native is hidden exactly when it
    /// was attached as a `builtin:<name>`, by default or by `-e builtin:<name>`.
    ///
    /// pi sets `extension.hidden = true` only in the `builtin:<name>` branch of
    /// `loadExtensionPaths` (`core/resource-loader.ts:741` @f1b2e77f5), which every `builtin:`
    /// path reaches — settings-resolved and `-e` (`cliEnabledExtensions`) alike, merged into one
    /// `extensionPaths` before `loadFinalExtensionSet` — and its `builtin: true` entries are
    /// `llama.cpp`, `codemode`, `tool-search` and `mcp` (`extensions/index.ts`); this fork ships
    /// three of them (`codemode` is deleted, docs/LEAN-FORK-PLAN.md). Everything else
    /// reaches pi as a file or package path and is listed. cyrup's `builtin:<name>` path is
    /// [`super::BuiltinSelection`] (EXT-094), the only way those natives are attached, so the
    /// expected state of every loaded native is derived from [`BUILTIN_EXTENSIONS`] — no per-native
    /// table — and the host's answer (`NativeExtension::is_hidden`, read through the startup
    /// `[Extensions]` list) must agree with it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_attached_native_is_hidden_exactly_when_pi_loads_its_counterpart_as_a_builtin() {
        assert_eq!(
            BUILTIN_EXTENSIONS,
            ["llama.cpp", "tool-search", "mcp"],
            "cyrup's builtin: names are pi's `builtInExtensions` minus `codemode`, which this fork does not ship"
        );
        let explicit: Vec<String> = BUILTIN_EXTENSIONS
            .iter()
            .map(|id| format!("builtin:{id}"))
            .collect();
        for (form, Loaded { loaded, listed, .. }) in [
            ("default", session_through_build_factory(true, false).await),
            (
                "-e builtin:<name>",
                session_with_extensions(true, &explicit).await,
            ),
        ] {
            for id in BUILTIN_EXTENSIONS {
                position(&loaded, id);
            }
            for id in &loaded {
                let builtin = BUILTIN_EXTENSIONS.contains(&id.as_str());
                assert_eq!(
                    !listed.contains(id),
                    builtin,
                    "{form}: {id} {} a builtin:<name> load, so pi {} it; listed {listed:?}",
                    if builtin { "is" } else { "is not" },
                    if builtin { "hides" } else { "lists" }
                );
            }
        }
    }

    /// `--no-extensions` drops it: pi's `builtin:llama.cpp` is a path in the tier the flag collapses
    /// (`package-manager.ts:972-974`, `resource-loader.ts:569-571`), which cyrup spells
    /// `is_ambient() -> true` (builder.rs:2647-2657). The same flag-off session keeps it, so this
    /// is the flag's effect and not a session that never had it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_extensions_drops_llama_in_every_mode() {
        for (mode, interactive) in [("interactive", true), ("rpc/print/json", false)] {
            let kept = session_through_build_factory(interactive, false)
                .await
                .loaded;
            assert!(kept.iter().any(|i| i == LLAMA_ID), "{mode}: {kept:?}");
            let dropped = session_through_build_factory(interactive, true)
                .await
                .loaded;
            assert!(
                !dropped.iter().any(|i| i == LLAMA_ID),
                "{mode}: --no-extensions drops the ambient llama.cpp built-in; got {dropped:?}"
            );
        }
    }

    /// TOOL-052 — `tool_search` is attached by `attach_native_extensions` (`extensions/index.ts:12`
    /// @v1.0.1) in every mode, registered inactive and `model-only`; `--no-extensions` drops it as
    /// it drops the other ambient built-ins. Read off a session built through `build_factory`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_search_is_attached_inactive_and_model_only_in_every_mode() {
        for (mode, interactive) in [("interactive", true), ("rpc/print/json", false)] {
            for no_extensions in [false, true] {
                let tmp = tempfile::tempdir().unwrap();
                let cwd = tmp.path().join("project");
                let agent_dir = tmp.path().join("agent");
                std::fs::create_dir_all(&cwd).unwrap();
                std::fs::create_dir_all(&agent_dir).unwrap();
                let (factory, target) = factory_at(&agent_dir, &cwd, interactive, no_extensions);
                let session: AgentSession = factory.build(target, None).await.unwrap();
                let row = session
                    .all_tools()
                    .into_iter()
                    .find(|row| row.name == "tool_search");
                if no_extensions {
                    assert!(row.is_none(), "{mode}: --no-extensions drops tool_search");
                    continue;
                }
                let row = row.unwrap_or_else(|| panic!("{mode}: tool_search is registered"));
                assert_eq!(row.exposure, cyrup_core::ToolExposure::ModelOnly, "{mode}");
                assert!(
                    !session
                        .active_tool_names()
                        .iter()
                        .any(|n| n == "tool_search"),
                    "{mode}: registered `defaultActive: false`"
                );
                assert!(
                    session
                        .services()
                        .ext_host
                        .loaded_ids()
                        .iter()
                        .any(|id| id.to_string() == cyrup_tool_search::EXTENSION_ID),
                    "{mode}: the extension is loaded"
                );
            }
        }
    }

    // ============================================================================================
    // MCP-604 — a `directTools: "search"` server's tools reach the model through `tool_search` and
    // through `mcp({ search })`, over the REAL launch wiring: the MCP adapter and `tool_search` are
    // the built-ins `attach_native_extensions` attaches, and the provider is scripted.
    // ============================================================================================

    /// The tool names each provider request declared, in request order.
    type Declared = Arc<std::sync::Mutex<Vec<Vec<String>>>>;

    /// A provider whose `n`th reply is `replies[n]` — a tool call (`Some((name, args))`) or text —
    /// recording the tool names every request declared.
    fn scripted(
        declared: &Declared,
        replies: Vec<Option<(&'static str, serde_json::Value)>>,
    ) -> Arc<FauxProvider> {
        let faux = Arc::new(FauxProvider::new());
        faux.set_response_steps(scripted_steps(declared, replies));
        faux
    }

    /// The response steps [`scripted`] installs.
    fn scripted_steps(
        declared: &Declared,
        replies: Vec<Option<(&'static str, serde_json::Value)>>,
    ) -> Vec<cyrup_provider::faux::FauxResponseStep> {
        use cyrup_core::StopReason;
        use cyrup_provider::faux::{
            FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
        };
        replies
            .into_iter()
            .map(|reply| {
                let seen = Arc::clone(declared);
                FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
                    seen.lock()
                        .unwrap()
                        .push(ctx.tools.iter().map(|tool| tool.name.clone()).collect());
                    match &reply {
                        Some((name, args)) => faux_assistant_message(
                            vec![faux_tool_call((*name).to_string(), args.clone())],
                            StopReason::ToolUse,
                        ),
                        None => faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
                    }
                })
            })
            .collect()
    }

    fn write_search_mode_world(agent_dir: &Path) {
        let definition = cyrup_mcp::config::ServerEntry {
            command: Some("echo".to_string()),
            ..cyrup_mcp::config::ServerEntry::default()
        };
        let hash = cyrup_mcp::registration::default_server_hasher(&definition).unwrap();
        std::fs::write(
            agent_dir.join("mcp.json"),
            serde_json::json!({
                "mcpServers": { "docs": { "command": "echo", "directTools": "search" } }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            agent_dir.join("mcp-cache.json"),
            serde_json::json!({
                "version": cyrup_mcp::registration::METADATA_CACHE_VERSION,
                "servers": { "docs": {
                    "configHash": hash,
                    "cachedAt": 4_102_444_800_000_u64,
                    "tools": [
                        {
                            "name": "lookup_widget",
                            "description": "Look up a widget by name.",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "name": { "type": "string" } },
                                "required": ["name"]
                            }
                        },
                        { "name": "rotate_logs", "description": "Rotate the log files." }
                    ],
                    "resources": []
                } }
            })
            .to_string(),
        )
        .unwrap();
    }

    /// A session over [`write_search_mode_world`], built through `build_factory`, with `extra`
    /// activated on top of the default tools (`--tools`).
    async fn search_mode_session(
        provider: Arc<FauxProvider>,
        extra: &[&str],
    ) -> (Arc<AgentSessionRuntime>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        write_search_mode_world(&agent_dir);
        let (factory, target) = factory_over(provider, &agent_dir, &cwd, false, false);
        // The runtime, as every mode builds its session: it shares the session (which binds the
        // self-handle the turn-boundary refresh reads, so a mid-run load reaches the next request)
        // and announces `session_start`, which is what builds the MCP runtime.
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let session = runtime.session().await;
        let mut active = session.active_tool_names();
        active.extend(extra.iter().map(|name| (*name).to_string()));
        session.set_active_tools_by_name(&active).await;
        (runtime, tmp)
    }

    /// The registered tools of a search-mode server are at the `deferred` exposure, in the server's
    /// namespace, and not in the active set: the model is not shown them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_search_mode_servers_tools_are_registered_deferred_and_not_declared() {
        let declared: Declared = Arc::default();
        let (runtime, _tmp) = search_mode_session(scripted(&declared, vec![None]), &[]).await;
        let session = runtime.session().await;
        let rows = session.all_tools();
        let row = rows
            .iter()
            .find(|row| row.name == "docs_lookup_widget")
            .expect("registered");
        assert_eq!(row.exposure, cyrup_core::ToolExposure::Deferred);
        assert_eq!(
            row.namespace
                .as_ref()
                .map(|namespace| namespace.name.as_str()),
            Some("mcp__docs")
        );
        assert!(
            !session
                .active_tool_names()
                .iter()
                .any(|name| name.starts_with("docs_")),
            "{:?}",
            session.active_tool_names()
        );
        let _ = session.prompt("hi").await.unwrap();
        session.wait_for_idle().await;
        let requests = declared.lock().unwrap().clone();
        assert!(
            names_at(&requests, 0)
                .iter()
                .all(|name| !name.starts_with("docs_")),
            "a deferred tool is not in the request: {:?}",
            names_at(&requests, 0)
        );
        assert!(
            names_at(&requests, 0).iter().any(|name| name == "mcp"),
            "the gateway is: {:?}",
            names_at(&requests, 0)
        );
    }

    /// MCP-604 `verify`: a search-mode MCP tool is findable by `tool_search` while inactive and is
    /// activated by it; the very next request declares it, with the cached schema.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_search_finds_a_search_mode_mcp_tool_and_the_next_request_declares_it() {
        let declared: Declared = Arc::default();
        let provider = scripted(
            &declared,
            vec![
                Some(("tool_search", serde_json::json!({ "query": "widget" }))),
                None,
            ],
        );
        let (runtime, _tmp) = search_mode_session(provider, &["tool_search"]).await;
        let session = runtime.session().await;
        let _ = session.prompt("find the widget tool").await.unwrap();
        session.wait_for_idle().await;

        let requests = declared.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            names_at(&requests, 0)
                .iter()
                .any(|name| name == "tool_search")
        );
        assert!(
            !names_at(&requests, 0)
                .iter()
                .any(|name| name == "docs_lookup_widget")
        );
        assert!(
            names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_lookup_widget"),
            "{:?}",
            names_at(&requests, 1)
        );
        assert!(
            !names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_rotate_logs"),
            "only the match is loaded: {:?}",
            names_at(&requests, 1)
        );
    }

    /// The gateway is the adapter's own entry point: a `mcp({ search })` that matches a search-mode
    /// tool loads it, so the very next request declares it, and the model is told so.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mcp_search_loads_a_search_mode_tool_for_the_next_request() {
        let declared: Declared = Arc::default();
        let provider = scripted(&declared, Vec::new());
        let (runtime, _tmp) = search_mode_session(Arc::clone(&provider), &[]).await;
        let session = runtime.session().await;

        // The MCP runtime builds in the background after `session_start`; a call that lands first
        // waits for the build. Ask for the gateway's status until it answers as a mode.
        let mut ready = false;
        for _ in 0..200 {
            provider.set_response_steps(scripted_steps(
                &declared,
                vec![Some(("mcp", serde_json::json!({}))), None],
            ));
            let _ = session.prompt("status").await.unwrap();
            session.wait_for_idle().await;
            let status = last_tool_result(&session, "mcp").await;
            if status
                .1
                .as_ref()
                .is_none_or(|details| details["error"] != "not_initialized")
            {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(ready, "the MCP runtime never came up");
        declared.lock().unwrap().clear();

        provider.set_response_steps(scripted_steps(
            &declared,
            vec![
                Some(("mcp", serde_json::json!({ "search": "widget" }))),
                None,
            ],
        ));
        let _ = session.prompt("find the widget tool").await.unwrap();
        session.wait_for_idle().await;

        let requests = declared.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(
            !names_at(&requests, 0)
                .iter()
                .any(|name| name == "docs_lookup_widget")
        );
        assert!(
            names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_lookup_widget"),
            "{:?}",
            names_at(&requests, 1)
        );
        assert!(
            !names_at(&requests, 1)
                .iter()
                .any(|name| name == "docs_rotate_logs")
        );
        let (content, details) = last_tool_result(&session, "mcp").await;
        let text = match content.first() {
            Some(cyrup_core::Content::Text { text, .. }) => text.to_string(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(
            text.starts_with("Activated as direct tools: docs_lookup_widget.\n\n"),
            "{text}"
        );
        assert_eq!(
            details.as_ref().unwrap()["activated"],
            serde_json::json!(["docs_lookup_widget"])
        );
    }

    /// The tool names request `index` declared (empty when there was no such request).
    fn names_at(requests: &[Vec<String>], index: usize) -> &[String] {
        requests.get(index).map_or(&[], Vec::as_slice)
    }

    /// The newest `tool_name` tool result of `session`: its content and details.
    pub(super) async fn last_tool_result(
        session: &AgentSession,
        tool_name: &str,
    ) -> (Vec<cyrup_core::Content>, Option<serde_json::Value>) {
        session
            .messages()
            .await
            .into_iter()
            .rev()
            .find_map(|message| match message {
                cyrup_core::Message::ToolResult {
                    tool_name: name,
                    content,
                    details,
                    ..
                } if name == tool_name => Some((content, details)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a `{tool_name}` tool result"))
    }

    #[test]
    fn scoped_active_model_prefers_saved_default_in_scope_else_first() {
        // Pi `buildSessionOptions` (main.ts:394-414): saved default in scope wins; else scoped[0].
        let a = scoped("anthropic", "");
        let o = scoped("openai", "");
        let scope = vec![a.clone(), o.clone()];

        // Saved default IS in scope (case-insensitive) → it is chosen, even though it is not first.
        let picked = pick_scoped_active_model(
            &scope,
            Some(&o.model.provider.as_str().to_uppercase()),
            Some(&o.model.id.as_str().to_uppercase()),
        )
        .expect("a pick");
        assert_eq!(picked.model.provider, o.model.provider);
        assert_eq!(picked.model.id, o.model.id);

        // Saved default NOT in scope → fall back to the first scoped model.
        let picked =
            pick_scoped_active_model(&scope, Some("together"), Some("nope")).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // No saved default → the first scoped model.
        let picked = pick_scoped_active_model(&scope, None, None).expect("a pick");
        assert_eq!(picked.model.provider, a.model.provider);

        // An empty scope yields nothing to pick.
        assert!(pick_scoped_active_model(&[], Some("openai"), Some("gpt-4o")).is_none());
    }

    // ============================================================================================
    // SUBA-166's blast radius — a native built-in that REFUSES to be built is quarantined, not
    // carried out of the launch path.
    //
    // `load_subagent_extension_config` refuses a `config.json` that failed validation while
    // declaring a policy key, and that refusal used to leave `attach_native_extensions` as an
    // `anyhow::Error`: no session at all, and not one of the other six built-ins attempted. pi
    // loses the one extension — the factory's throw is caught, `load.discard()`ed and recorded as
    // `Failed to load extension: <message>`
    // (`pi/packages/coding-agent/src/core/extensions/loader.ts:613-630`, `:655` @v1.0.1) —
    // and the session is built without it.
    //
    // Every test here drives the production seam, `build_factory`, and reads what the extension
    // host actually loaded plus the diagnostics the bin's checkpoint consumes.
    // ============================================================================================

    // ============================================================================================
    // The same quarantine at the SAME attach point, for the intercom built-in.
    //
    // Upstream's intercom factory opens with `loadConfig()` and `getAskTimeoutMs()`
    // (`pi-intercom/index.ts:647-649` @v0.16.0), both of which THROW on bad input
    // (`config.ts:98-159` and `:18`). They throw from inside the default-exported factory, which is
    // the body pi's loader wraps in `try` — so a malformed intercom config costs upstream that one
    // extension and nothing else. cyrup carried the error out of `attach_native_extensions`
    // instead, aborting the entire launch.
    // ============================================================================================

    /// The intercom built-in's own id, as the placeholder carries it.
    const INTERCOM_ID: &str = cyrup_intercom::EXTENSION_ID;

    /// Write `<agent_dir>/intercom/config.json`. As with subagents, its mere PRESENCE is the
    /// install signal (`cyrup_intercom::extension::is_installed` is
    /// `env_truthy(INSTALL_ENV_VAR) || config_path(..).exists()`), so one file both opts the
    /// extension in and supplies the bad input — the control and the refusal differ only in the
    /// file's CONTENTS, never in whether intercom was reachable at all.
    async fn launch_with_intercom_config(json: &str) -> (Vec<String>, Vec<RuntimeDiagnostic>) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        let dir = agent_dir.join("intercom");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), json).unwrap();
        let (factory, target) = factory_at(&agent_dir, &cwd, false, false);
        let runtime = AgentSessionRuntime::create(factory, target)
            .await
            .expect("a malformed intercom config must not abort the launch");
        let diagnostics = runtime.diagnostics().await;
        let loaded = runtime
            .session()
            .await
            .services()
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        (loaded, diagnostics)
    }

    /// Unparseable JSON — `load_config`'s `parse_config` arm, which carries pi's path-prefixed
    /// message (`config.ts:94-96`'s catch).
    const REFUSED_INTERCOM_CONFIG: &str = "{ this is not valid json";
    /// The same file, valid. `enabled` defaults to `true` (`config.rs:118`), so this both installs
    /// and enables intercom and the control actually attaches it.
    const ACCEPTED_INTERCOM_CONFIG: &str = r#"{"confirmSend": true}"#;

    /// The intercom headline, built the same way as the subagents one: the loaded set after the
    /// refusal is EXACTLY the control launch's set minus `cyrup-intercom`.
    ///
    /// Before the fix this test could not even reach its assertions — `AgentSessionRuntime::create`
    /// returned `Err`, because the refusal propagated out of `attach_native_extensions` and there
    /// was no session at all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_malformed_intercom_config_costs_the_launch_only_that_extension() {
        let (clean, clean_diags) = launch_with_intercom_config(ACCEPTED_INTERCOM_CONFIG).await;
        assert!(
            clean.iter().any(|i| i == INTERCOM_ID),
            "the control launch must actually attach intercom, or this test proves nothing; got {clean:?}"
        );
        assert!(
            clean_diags.is_empty(),
            "a valid intercom config is not a diagnostic: {clean_diags:?}"
        );

        let (refused, _) = launch_with_intercom_config(REFUSED_INTERCOM_CONFIG).await;
        let expected: Vec<String> = clean
            .iter()
            .filter(|i| i.as_str() != INTERCOM_ID)
            .cloned()
            .collect();
        assert_eq!(
            refused, expected,
            "a malformed intercom config must quarantine intercom and NOTHING else (clean: {clean:?})"
        );
    }

    /// The reporting half. A silent quarantine would pass the test above while losing the operator's
    /// only clue, and would be strictly worse than the launch abort it replaces.
    ///
    /// The message must still name the config's PATH, because that is the whole point of `ICOM-044`
    /// (a malformed config errors with the path rather than failing closed silently) and the
    /// quarantine must not undo it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_intercom_refusal_is_reported_as_pis_own_fatal_extension_load_failure() {
        let (_, diags) = launch_with_intercom_config(REFUSED_INTERCOM_CONFIG).await;
        let errors: Vec<&RuntimeDiagnostic> =
            diags.iter().filter(|d| d.severity == "error").collect();
        assert_eq!(
            errors.len(),
            1,
            "expected exactly one fatal diagnostic, got {diags:?}"
        );
        let only = errors.first().expect("one error");
        assert_eq!(only.source.as_deref(), Some("extension"));
        assert!(
            only.message
                .starts_with(&format!("Failed to load extension \"{INTERCOM_ID}\": ")),
            "pi's shape is `Failed to load extension \"<id>\": <err>`; got {}",
            only.message
        );
        for needle in ["Failed to load intercom config at", "config.json"] {
            assert!(
                only.message.contains(needle),
                "ICOM-044's path-naming message is carried verbatim; `{needle}` is missing from {}",
                only.message
            );
        }
    }

    // ============================================================================================
    // EXT-094 — the built-ins are extension resources named `builtin:<name>`: the user
    // `extensions` setting disables one by name, and `-e builtin:<name>` loads one explicitly,
    // also under `--no-extensions` (pi `core/package-manager.ts` `resolve()` /
    // `resolveExtensionSources`, `core/resource-loader.ts` `noExtensions ? cliEnabledExtensions`).
    // ============================================================================================

    /// What [`launch_with_builtins`] observed: the loaded ids, the runtime diagnostics, and every
    /// entry of the `[Extension issues]` panel (`startup_diagnostics.extensions`, as
    /// `<path>: <error>`), which also holds the non-fatal ones `diagnostics` leaves out — a `-e`
    /// path that does not exist among them.
    struct BuiltinLaunch {
        loaded: Vec<String>,
        diagnostics: Vec<RuntimeDiagnostic>,
        issues: Vec<String>,
    }

    /// A hermetic launch through `build_factory` + `AgentSessionRuntime::create`, with the global
    /// `settings.json` holding `settings` (when given), `--no-extensions` set by `no_extensions`,
    /// and `extensions` passed as `-e`.
    async fn launch_with_builtins(
        settings: Option<&str>,
        no_extensions: bool,
        extensions: &[&str],
    ) -> BuiltinLaunch {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        if let Some(json) = settings {
            std::fs::write(agent_dir.join("settings.json"), json).unwrap();
        }
        let (factory, target) = factory_with(
            Arc::new(FauxProvider::new()),
            &agent_dir,
            &cwd,
            false,
            |config| {
                config.no_extensions = no_extensions;
                config.extra_extension_paths =
                    extensions.iter().map(std::path::PathBuf::from).collect();
            },
        );
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let diagnostics = runtime.diagnostics().await;
        let session = runtime.session().await;
        let services = session.services();
        let loaded = services
            .ext_host
            .loaded_ids()
            .iter()
            .map(ToString::to_string)
            .collect();
        let issues = services
            .startup_diagnostics
            .extensions
            .iter()
            .map(|d| format!("{}: {}", d.path.display(), d.error))
            .collect();
        BuiltinLaunch {
            loaded,
            diagnostics,
            issues,
        }
    }

    /// `ids` without `id`.
    fn without(ids: &[String], id: &str) -> Vec<String> {
        ids.iter().filter(|i| i.as_str() != id).cloned().collect()
    }

    /// Verify, first clause: a settings entry removes ONLY llama.cpp. The loaded set is exactly the
    /// default launch's minus `llama.cpp`, so every other built-in (mcp, tool-search, codemode's
    /// absence-or-presence included) is untouched; `-builtin:mcp` shows the name is what decides.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_settings_entry_disables_only_the_built_in_it_names() {
        let default = launch_with_builtins(None, false, &[]).await.loaded;
        assert!(
            default.iter().any(|i| i == LLAMA_ID) && default.iter().any(|i| i == MCP_ID),
            "the control launch loads both built-ins; got {default:?}"
        );
        for id in [LLAMA_ID, MCP_ID] {
            let settings = format!(r#"{{"extensions": ["-builtin:{id}"]}}"#);
            let run = launch_with_builtins(Some(&settings), false, &[]).await;
            assert_eq!(
                run.loaded,
                without(&default, id),
                "`-builtin:{id}` disables {id} and nothing else"
            );
            assert!(
                run.diagnostics.is_empty() && run.issues.is_empty(),
                "no diagnostic for a setting: {:?} {:?}",
                run.diagnostics,
                run.issues
            );
        }
    }

    /// Verify, second clause: `--no-extensions -e builtin:llama.cpp` keeps only it. The loaded set
    /// is exactly the bare `--no-extensions` launch's plus `llama.cpp`, so no other built-in comes
    /// back with it — and the `builtin:` entry never reaches the file discovery, which would report
    /// it as a missing path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_extensions_with_an_explicit_built_in_keeps_only_that_built_in() {
        let bare = launch_with_builtins(None, true, &[]).await.loaded;
        for id in BUILTIN_EXTENSIONS {
            assert!(
                !bare.iter().any(|i| i == id),
                "--no-extensions drops {id}; got {bare:?}"
            );
        }
        for id in BUILTIN_EXTENSIONS {
            let path = format!("builtin:{id}");
            let run = launch_with_builtins(None, true, &[&path]).await;
            let loaded = &run.loaded;
            assert!(
                loaded.iter().any(|i| i == id),
                "-ne -e {path} loads {id}; got {loaded:?}"
            );
            assert_eq!(
                without(loaded, id),
                bare,
                "-ne -e {path} adds {id} and nothing else"
            );
            assert!(
                run.diagnostics.is_empty() && run.issues.is_empty(),
                "-ne -e {path} reports nothing, and is never looked up as a file: {:?} {:?}",
                run.diagnostics,
                run.issues
            );
        }
    }

    /// `-e builtin:<name>` is in pi's `cliEnabledExtensions`, merged ahead of the settings-resolved
    /// set, so the command line loads a built-in the setting disabled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_explicit_built_in_loads_over_a_settings_entry() {
        let settings = r#"{"extensions": ["-builtin:llama.cpp"]}"#;
        let loaded = launch_with_builtins(Some(settings), false, &["builtin:llama.cpp"])
            .await
            .loaded;
        assert!(
            loaded.iter().any(|i| i == LLAMA_ID),
            "-e builtin:llama.cpp wins over -builtin:llama.cpp; got {loaded:?}"
        );
    }

    /// A name that is no built-in is pi's `Unknown built-in extension: builtin:<name>` load error,
    /// reported against that path — under `--no-extensions` too, since it is an explicit `-e` path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unknown_built_in_is_a_load_error() {
        for no_extensions in [false, true] {
            let diags = launch_with_builtins(None, no_extensions, &["builtin:nope"])
                .await
                .diagnostics;
            let errors: Vec<&RuntimeDiagnostic> =
                diags.iter().filter(|d| d.severity == "error").collect();
            assert_eq!(errors.len(), 1, "-ne={no_extensions}: {diags:?}");
            assert_eq!(
                errors.first().map(|d| d.message.as_str()),
                Some(
                    "Failed to load extension \"builtin:nope\": \
                     Unknown built-in extension: builtin:nope"
                ),
                "-ne={no_extensions}"
            );
        }
    }

    // ============================================================================================
    // `--tools` / `--exclude-tools` — the allowlist bounds the REGISTRY, not only the initial
    // selection (pi `_isAllowedTool`, `agent-session.ts` `_refreshToolRegistry` @v1.0.4 drops a
    // disallowed built-in before the registry is built, so `setActiveToolsByName` and a tool call can
    // only ever reach what the flags allow).
    //
    // Driven over the real launch wiring: `build_factory` attaches the permission system (policy file
    // present, so it installs) and `tool_search`; the permission extension's `before_agent_start`
    // runs on every turn and names every registry tool it exposes. The scripted model CALLS the tool
    // under test, so "can this tool be reached" is answered by the result the session hands back for
    // a call the model actually made — and a call that reaches `bash` is visible in the transcript.
    //
    // **[CYRUP-DELTA]** pi walks the registry from inside a `codemode` script (`tools.*`); this fork
    // ships no codemode (docs/LEAN-FORK-PLAN.md), so the same registry-bound property is pinned from
    // the model's own tool call, which is the only other caller a session has.
    // ============================================================================================

    /// A hermetic world whose `cyrup-permissions.jsonc` makes the permission system install.
    fn permission_world() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        permission_world_with(
            r#"{ "defaultPolicy": { "tools": "allow", "bash": "allow", "mcp": "allow", "skills": "allow" } }"#,
        )
    }

    /// [`permission_world`] over the given policy file.
    fn permission_world_with(
        policy: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let agent_dir = tmp.path().join("agent");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Everything allowed: the permission system exposes every registry tool it is asked about,
        // which is the case that matters here (a policy that denied `bash` would hide the defect).
        std::fs::write(agent_dir.join("cyrup-permissions.jsonc"), policy).unwrap();
        (tmp, agent_dir, cwd)
    }

    /// The built-in tool names a session may not expose when the flags allow only `read`, `grep` and
    /// `bash`.
    const NARROWED_OUT: [&str; 4] = ["edit", "write", "find", "ls"];

    /// The command every probe makes the model run through `bash`: it leaves `reached-bash` in the
    /// session's cwd, which is the only unambiguous evidence that the command ran — a block reason
    /// quotes the command back, so an echoed marker cannot tell the two apart.
    const PROBE_COMMAND: &str = "touch reached-bash";

    /// What a probe run produced.
    struct Probe {
        runtime: Arc<AgentSessionRuntime>,
        /// The tool names each provider request declared, in request order.
        requests: Vec<Vec<String>>,
        /// The text of the `bash` tool result the session produced for the model's call.
        bash_result: String,
        /// Whether that result was an error result.
        bash_is_error: bool,
        /// The session's cwd — where [`PROBE_COMMAND`] leaves its marker.
        cwd: std::path::PathBuf,
        /// Kept alive so the hermetic world outlives the session.
        _tmp: tempfile::TempDir,
    }

    impl Probe {
        /// Whether the probe command actually ran.
        fn command_ran(&self) -> bool {
            self.cwd.join("reached-bash").exists()
        }
    }

    /// Launch a session in `world` with the flags `configure` sets, make the model call `bash` with
    /// [`PROBE_COMMAND`], and report what the session answered.
    async fn probe_bash(
        world: (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf),
        configure: impl FnOnce(&mut SessionConfig),
    ) -> Probe {
        let (tmp, agent_dir, cwd) = world;
        let declared: Declared = Arc::default();
        let provider = scripted(
            &declared,
            vec![
                Some(("bash", serde_json::json!({ "command": PROBE_COMMAND }))),
                None,
            ],
        );
        let (factory, target) = factory_with(provider, &agent_dir, &cwd, false, configure);
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let session = runtime.session().await;
        let _ = session.prompt("probe").await.unwrap();
        session.wait_for_idle().await;

        let (content, _, is_error) = session
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
                } if tool_name == "bash" => Some((content, (), is_error)),
                _ => None,
            })
            .expect("a `bash` tool result");
        let bash_result = content
            .iter()
            .filter_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        Probe {
            runtime,
            requests: declared.lock().unwrap().clone(),
            bash_result,
            bash_is_error: is_error,
            cwd,
            _tmp: tmp,
        }
    }

    /// `--tools read,bash`: a disallowed built-in is not in the registry, so neither the permission
    /// extension's per-turn `set_active_tools` nor a tool call can reach it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tools_allowlist_bounds_the_registry_the_permission_gate_reaches() {
        let probe = probe_bash(permission_world(), |cfg| {
            cfg.tools = Some(vec!["read".into(), "grep".into(), "bash".into()]);
        })
        .await;
        let session = probe.runtime.session().await;

        for request in &probe.requests {
            for name in NARROWED_OUT {
                assert!(
                    !request.iter().any(|declared| declared == name),
                    "`{name}` was declared to the model under --tools read,grep,bash: {request:?}"
                );
            }
        }
        let registered: Vec<String> = session
            .all_tools()
            .into_iter()
            .map(|row| row.name)
            .collect();
        let active = session.active_tool_names();
        for name in NARROWED_OUT {
            assert!(
                !registered.iter().any(|n| n == name),
                "`{name}` is in the registry: {registered:?}"
            );
            assert!(
                !active.iter().any(|n| n == name),
                "`{name}` is active: {active:?}"
            );
        }
        assert!(
            registered.iter().any(|n| n == "read") && registered.iter().any(|n| n == "bash"),
            "the allowlisted tools are in the registry: {registered:?}"
        );
        // `bash` IS allowed here, so the call is what proves the world is live: the command ran.
        assert!(
            !probe.bash_is_error && probe.command_ran(),
            "an allowlisted `bash` call did not run: {:?}",
            probe.bash_result
        );
    }

    /// The same bound from the other side: a tool the allowlist does not name is unreachable by CALL
    /// as well as by declaration — the session answers the call itself.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tool_the_allowlist_drops_is_unreachable_to_a_call_as_well_as_to_the_model() {
        let probe = probe_bash(permission_world(), |cfg| {
            cfg.tools = Some(vec!["read".into(), "grep".into()]);
        })
        .await;
        assert!(
            probe.bash_is_error,
            "the call to a dropped tool is an error result: {:?}",
            probe.bash_result
        );
        assert!(
            !probe.command_ran(),
            "the command ran: {:?}",
            probe.bash_result
        );
        let session = probe.runtime.session().await;
        assert!(
            !session.all_tools().iter().any(|row| row.name == "bash"),
            "`bash` is in the registry under --tools read,grep"
        );
        for request in &probe.requests {
            assert!(
                !request.iter().any(|name| name == "bash"),
                "`bash` was declared to the model: {request:?}"
            );
        }
    }

    /// A policy that denies the whole `bash` tool: through the real permission extension and the real
    /// session builder the call never runs, and the reason names the policy. The tool is not declared
    /// to the model either — the gate hides what it denies.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deny_rule_for_bash_blocks_the_call_and_hides_the_tool_from_the_model() {
        let world = permission_world_with(
            r#"{ "defaultPolicy": { "tools": "allow", "bash": "allow", "mcp": "allow", "skills": "allow" },
                 "tools": { "bash": "deny" } }"#,
        );
        let probe = probe_bash(world, |_| {}).await;
        assert!(
            probe.bash_is_error,
            "a denied tool call is an error result: {:?}",
            probe.bash_result
        );
        assert!(
            !probe.command_ran(),
            "the command ran: {:?}",
            probe.bash_result
        );
        // A tool denied outright is not merely blocked: the gate takes it out of the reachable set,
        // so the call answers like a call to a tool that is not there.
        assert!(
            probe.bash_result.contains("not found"),
            "the denied tool is still reachable: {:?}",
            probe.bash_result
        );
        // The gate HIDES what it denies rather than unregistering it: the built-in stays in the
        // registry, is not active, is not declared, and a call that names it gets nothing.
        let session = probe.runtime.session().await;
        assert!(
            session.all_tools().iter().any(|row| row.name == "bash"),
            "a denied built-in was dropped from the registry, which is a different defect"
        );
        assert!(
            !session.active_tool_names().iter().any(|n| n == "bash"),
            "a denied `bash` is active"
        );
        for request in &probe.requests {
            assert!(
                !request.iter().any(|name| name == "bash"),
                "a denied `bash` is still declared to the model: {request:?}"
            );
        }
    }

    /// A command-level deny reaches the call itself: the result carries the policy's text, and the
    /// command never runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_deny_rule_for_one_bash_command_rejects_that_call() {
        let world = permission_world_with(
            r#"{ "defaultPolicy": { "tools": "allow", "bash": "allow", "mcp": "allow", "skills": "allow" },
                 "bash": { "touch *": "deny" } }"#,
        );
        let probe = probe_bash(world, |_| {}).await;
        assert!(
            probe.bash_is_error,
            "a command-level deny is an error result: {:?}",
            probe.bash_result
        );
        assert!(
            !probe.command_ran(),
            "the command ran: {:?}",
            probe.bash_result
        );
        assert!(
            probe.bash_result.contains("policy-enforced"),
            "the block reason is not the policy's: {:?}",
            probe.bash_result
        );
    }

    /// An `ask` with no UI to answer it (this session is headless, like `-p` and `--mode json`)
    /// becomes a block, and the result says why: the call, and what to do about it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_headless_ask_blocks_the_call_with_a_reason_the_model_can_read() {
        let world = permission_world_with(
            r#"{ "defaultPolicy": { "tools": "allow", "bash": "ask", "mcp": "allow", "skills": "allow" } }"#,
        );
        let probe = probe_bash(world, |_| {}).await;
        assert!(
            probe.bash_is_error,
            "an unanswerable `ask` is an error result: {:?}",
            probe.bash_result
        );
        assert!(
            !probe.command_ran(),
            "the command ran: {:?}",
            probe.bash_result
        );
        assert!(
            probe
                .bash_result
                .contains("requires approval, but no interactive UI is available."),
            "the block reason does not name the missing UI: {:?}",
            probe.bash_result
        );
    }

    /// `--exclude-tools bash`: the denylist bounds the registry the same way, with the rest of the
    /// allowlist intact.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exclude_tools_bounds_the_registry_the_permission_gate_reaches() {
        let probe = probe_bash(permission_world(), |cfg| {
            cfg.exclude_tools = vec!["bash".into()];
            // `--tools` names `bash` too: the denylist wins over the allowlist (pi `_isAllowedTool`).
            cfg.tools = Some(vec!["read".into(), "bash".into(), "edit".into()]);
        })
        .await;
        let session = probe.runtime.session().await;

        for request in &probe.requests {
            assert!(
                !request.iter().any(|declared| declared == "bash"),
                "`bash` was declared under --exclude-tools bash: {request:?}"
            );
        }
        assert!(
            !session.all_tools().iter().any(|row| row.name == "bash"),
            "`bash` is in the registry"
        );
        assert!(
            probe.bash_is_error && !probe.command_ran(),
            "`bash` answered despite the denylist: {:?}",
            probe.bash_result
        );
        assert!(
            session.all_tools().iter().any(|row| row.name == "edit"),
            "the denylist took more than it names"
        );
    }

    /// The tool names and the system prompt of every request, in request order.
    type Requests = Arc<std::sync::Mutex<Vec<(Vec<String>, String)>>>;

    /// A provider that answers with text and records what each request declared and said.
    fn recording(requests: &Requests) -> Arc<FauxProvider> {
        use cyrup_core::StopReason;
        use cyrup_provider::faux::{FauxResponseStep, faux_assistant_message, faux_text};
        let faux = Arc::new(FauxProvider::new());
        let seen = Arc::clone(requests);
        faux.set_response_steps(vec![FauxResponseStep::factory(
            move |ctx, _opts, _state, _model| {
                seen.lock().unwrap().push((
                    ctx.tools.iter().map(|tool| tool.name.clone()).collect(),
                    ctx.system_prompt.clone().unwrap_or_default(),
                ));
                faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
            },
        )]);
        faux
    }

    /// What the first request of a session over `policy` (and, when given, `settings.json`) declared
    /// and said, and what the session knew about the tools it declared.
    struct FirstRequest {
        /// The tool names the request declared, sorted.
        declared: Vec<String>,
        /// The system prompt the request carried.
        prompt: String,
        /// The declared tools that have a prompt snippet. pi lists exactly these in the prompt's
        /// `<tools>` section: `visibleTools = declaredTools.filter((name) => !!toolSnippets[name])`
        /// (`core/system-prompt.ts:150-159` @f1b2e77f5). A declared tool with no snippet reaches the
        /// model through the request alone.
        with_snippet: Vec<String>,
        /// Every tool the session registered, active or not.
        registered: Vec<String>,
    }

    /// [`first_request_in`] in the world the process environment makes.
    async fn first_request(policy: &str, settings: Option<&str>) -> FirstRequest {
        first_request_in(policy, settings, |_| {}).await
    }

    /// The first request of a session over `policy`, with the subagents extension installed by file
    /// when `subagents` is set (`CYRUP_SUBAGENTS=1` installs it from the environment too) and the
    /// launch flags `configure` sets.
    async fn first_request_in(
        policy: &str,
        settings: Option<&str>,
        configure: impl FnOnce(&mut SessionConfig),
    ) -> FirstRequest {
        let (tmp, agent_dir, cwd) = permission_world_with(policy);
        if let Some(settings) = settings {
            std::fs::write(agent_dir.join("settings.json"), settings).unwrap();
        }
        let requests: Requests = Arc::default();
        let (factory, target) =
            factory_with(recording(&requests), &agent_dir, &cwd, false, configure);
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let session = runtime.session().await;
        let _ = session.prompt("go").await.unwrap();
        session.wait_for_idle().await;
        let (mut declared, prompt) = requests.lock().unwrap().first().cloned().unwrap();
        declared.sort();
        let with_snippet = declared
            .iter()
            .filter(|name| {
                session
                    .tool_definition(name)
                    .is_some_and(|tool| tool.prompt_snippet.is_some())
            })
            .cloned()
            .collect();
        let mut registered: Vec<String> = session
            .all_tools()
            .into_iter()
            .map(|row| row.name)
            .collect();
        registered.sort();
        drop(tmp);
        FirstRequest {
            declared,
            prompt,
            with_snippet,
            registered,
        }
    }

    /// The tools of `declared` that have no prompt snippet, which the `<tools>` section cannot list.
    fn without_snippet(first: &FirstRequest) -> Vec<&String> {
        first
            .declared
            .iter()
            .filter(|name| !first.with_snippet.contains(name))
            .collect()
    }

    /// The tool names the `<tools>` section of a system prompt lists, in order.
    fn listed_tools(prompt: &str) -> Vec<String> {
        let section = prompt
            .split("<tools>")
            .nth(1)
            .and_then(|rest| rest.split("</tools>").next())
            .unwrap_or_else(|| panic!("no <tools> section in {prompt:?}"));
        section
            .lines()
            .filter_map(|line| line.strip_prefix("- "))
            .filter_map(|line| line.split(':').next())
            .map(str::to_owned)
            .collect()
    }

    /// A policy that leaves `skills` at its default (`ask`), which the permission system answers by
    /// hiding the skills from the prompt: the case in which it returns a prompt of its own.
    const POLICY_SKILLS_ASK: &str = r#"{ "defaultPolicy": { "tools": "allow", "bash": "allow" } }"#;

    /// The first prompt of a session with an armed permission system is the prompt of the tools the
    /// first request declares. The system prompt the model was sent listed six tools and the rules
    /// for them (`Use bash for file operations like ls, rg, find`) while the request declared twelve,
    /// including `tool_search`, because the permission system returned a sanitized copy of the prompt
    /// it was handed, built before it changed the active tools.
    ///
    /// "The tools the request declares" is pi's rule, which lists the declared tools that carry a
    /// prompt snippet. In a session with nothing installed that is all of them. The subagents
    /// extension declares two tools with no snippet, `subagent` and `bg_wait`, and nothing else
    /// without one, so this holds in a world that has it too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_first_prompt_lists_the_tools_the_first_request_declares() {
        let first = first_request(POLICY_SKILLS_ASK, None).await;
        let mut listed = listed_tools(&first.prompt);
        listed.sort();
        let prompt = &first.prompt;
        assert_eq!(
            listed, first.with_snippet,
            "the <tools> section lists the declared tools that carry a snippet, in:\n{prompt}"
        );
        assert!(
            without_snippet(&first)
                .iter()
                .all(|name| matches!(name.as_str(), "subagent" | "bg_wait")),
            "only the subagents extension declares a tool with no snippet: {:?}",
            without_snippet(&first)
        );
        assert!(
            first.declared.iter().any(|name| name == "tool_search"),
            "tool_search is active under this policy: {:?}",
            first.declared
        );
        // **[CYRUP-DELTA]** this fork ships no `codemode` tool (docs/LEAN-FORK-PLAN.md): neither the
        // request declares one nor does the prompt describe one.
        assert!(
            !first.declared.iter().any(|name| name == "codemode"),
            "a codemode tool is declared: {:?}",
            first.declared
        );
        assert!(
            !prompt.contains("codemode"),
            "the prompt still describes codemode:\n{prompt}"
        );
        assert!(
            !prompt.contains("Use bash for file operations like ls, rg, find"),
            "a rule for a tool that grep, find and ls replace is stale in:\n{prompt}"
        );
    }

    /// The tools `--exclude-tools` takes away in the narrowed-set tests: everything but `read` that
    /// carries a prompt rule, so a rule surviving in the prompt means the rule set and the declared
    /// set disagree.
    const NARROW_OUT: [&str; 6] = ["bash", "edit", "write", "find", "ls", "grep"];

    /// `cfg.exclude_tools = NARROW_OUT`.
    fn narrow_out(cfg: &mut SessionConfig) {
        cfg.exclude_tools = NARROW_OUT.iter().map(|name| (*name).to_string()).collect();
    }

    /// The same over a narrowed tool set — `--exclude-tools` where pi used `codemode.mode: only` —
    /// where the request declares `read` and none of the file tools: a prompt describing `bash`,
    /// `edit` and `write` to a model that was not given them is stale, whatever narrowed the set.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_first_prompt_of_a_narrowed_tool_set_lists_only_what_the_request_declares() {
        let first = first_request_in(POLICY_SKILLS_ASK, None, narrow_out).await;
        let prompt = &first.prompt;
        for name in NARROW_OUT {
            assert!(
                !first.declared.iter().any(|declared| declared == name),
                "`{name}` is declared under --exclude-tools: {:?}",
                first.declared
            );
        }
        assert!(
            first.declared.iter().any(|name| name == "read"),
            "the tool the flags left is declared: {:?}",
            first.declared
        );
        assert!(
            first.registered.iter().any(|name| name == "read"),
            "the declared tool is registered too: {:?}",
            first.registered
        );
        let mut listed = listed_tools(prompt);
        listed.sort();
        assert_eq!(
            listed, first.with_snippet,
            "the <tools> section of:\n{prompt}"
        );
        assert!(
            prompt.contains("Use read to examine"),
            "the rule for the tool that survives is missing from:\n{prompt}"
        );
        for stale in ["Use bash for", "Use edit for", "Use write only"] {
            assert!(!prompt.contains(stale), "`{stale}` is stale in:\n{prompt}");
        }
        assert!(
            !prompt.contains("codemode"),
            "the prompt still describes codemode:\n{prompt}"
        );
    }

    /// A caller that names a disallowed built-in to `set_active_tools_by_name` (the CLI/host path
    /// the permission extension's `set_active_tools` shares) activates nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_active_tools_by_name_cannot_reactivate_a_disallowed_builtin() {
        let (tmp, agent_dir, cwd) = permission_world();
        let (factory, target) = factory_with(
            Arc::new(FauxProvider::new()),
            &agent_dir,
            &cwd,
            false,
            |cfg| cfg.tools = Some(vec!["read".into(), "grep".into()]),
        );
        let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();
        let session = runtime.session().await;
        let everything: Vec<String> = ["read", "bash", "edit", "write", "grep", "find", "ls"]
            .iter()
            .map(|n| (*n).to_string())
            .collect();
        session.set_active_tools_by_name(&everything).await;
        let mut active = session.active_tool_names();
        active.sort();
        assert_eq!(active, vec!["grep".to_string(), "read".to_string()]);
        drop(tmp);
    }
}
