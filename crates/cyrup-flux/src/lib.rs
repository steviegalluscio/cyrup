//! cyrup-flux — Flux: a structured, file-persisted AI development pipeline
//! (`new → ask → split → aug → exec → qa → tests → commit → create-pr`), ported from
//! code-puppy's `flux_bootstrap` plugin onto cyrup as a single
//! `cyrup_ext::native::NativeExtension`.
//!
//! Two mechanisms, both cyrup-native replacements for upstream machinery:
//!   * the 15 pipeline commands are cyrup PROMPT TEMPLATES bundled in `resources/prompts/flux/`,
//!     EMBEDDED in the binary at build time (`build.rs` → [`bundle`]), materialised under
//!     `<agent_dir>/flux/resources` by the port of code-puppy's `installer.py` ([`install`]) and
//!     contributed to every session via `ResourcesDiscover` — replacing code-puppy's
//!     `customizable_commands` dispatcher;
//!   * `status` / `cheatsheet` / `about` are NATIVE COMMANDS — replacing code-puppy's
//!     `exec:` frontmatter directive, which cyrup deliberately does not support.
//!
//! State on disk (`~/.flux/<flattened-cwd>/`) stays byte-identical to code-puppy's so one
//! project's task tree is readable by both harnesses.
//!
//! # Upstream baseline
//!
//! Ported from `mpfaffenberger/code_puppy_core_plugins` @ **`v0.0.6`** (cloned as
//! `tmp/code_puppy_core_plugins`; the plugin is the `code_puppy_core_plugins/flux_bootstrap/`
//! package inside it), whose `flux_bootstrap` tree is byte-identical through `v0.0.40` — the tag
//! the in-source `@v0.0.40` anchors name. The host-side names the prompts call (the
//! `invoke_agent` tool the pipeline's `subagent` rename maps from) are `mpfaffenberger/code_puppy`
//! @ **`v0.0.720`** (cloned as `tmp/code_puppy`). Every upstream citation in this crate resolves
//! with `git -C tmp/<that clone> show <tag>:<path>`; the design doc these modules cite
//! as "port doc §N" is `spec/flux.md`.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![forbid(unsafe_code)]

pub mod ask_tool;
pub mod bundle;
pub mod extension;
pub mod install;
pub mod overlay;
pub mod render_about;
pub mod render_cheatsheet;
pub mod render_status;
pub mod resources;
pub mod state;

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

/// Construct the flux extension.
///
/// `agent_dir` is the binary's resolved agent directory (`ConfigDirs::agent_dir`, the same value
/// every other native extension is handed at `crates/cyrup/src/session_launch.rs`): the embedded
/// prompt/skill bundle is materialised under `<agent_dir>/flux/resources` (FLUX-001) unless
/// `CYRUP_FLUX_RESOURCES_DIR` points at a vendored tree — see [`resources::BundledRoot`].
#[must_use]
pub fn flux_extension(agent_dir: &Path) -> Arc<extension::FluxExtension> {
    flux_extension_with_root(resources::BundledRoot::resolve(agent_dir))
}

/// [`flux_extension`] with the bundled root already decided — the seam a test uses to point the
/// extension at a scratch directory without touching the process environment.
#[must_use]
pub fn flux_extension_with_root(root: resources::BundledRoot) -> Arc<extension::FluxExtension> {
    Arc::new(extension::FluxExtension {
        id: cyrup_core::ExtensionId::from("cyrup-flux"),
        host_services: Arc::new(cyrup_ext::host::HostServicesSlot::new()),
        root,
    })
}

/// [`flux_extension`] with the bundled root taken from the process environment.
///
/// [CYRUP-DELTA] upstream returns `None` inside a subagent CHILD — a child re-execs this binary in
/// Print/Json mode, and 15 templates plus a skill in every child's system prompt is noise for a
/// pipeline the child is not running. There is no child mode here: the marker (`CYRUP_SUBAGENT_CHILD`)
/// was set only by the subagents extension, which this fork deletes in step 4 of
/// `docs/LEAN-FORK-PLAN.md`. The `Option` and the injected-environment seam stay, since the bundled
/// root still resolves through the environment.
#[must_use]
pub fn flux_extension_for_env(agent_dir: &Path) -> Option<Arc<extension::FluxExtension>> {
    flux_extension_for_env_from(agent_dir, &|key| std::env::var_os(key))
}

/// [`flux_extension_for_env`] with the environment supplied — root resolution as a pure rule, the
/// same closure-shaped seam as [`resources::BundledRoot::resolve_from`] (which the bundled root is
/// resolved through here too), so a test pins the `None` branch without `set_var` (unsafe in
/// edition 2024; this crate forbids `unsafe`).
#[must_use]
pub fn flux_extension_for_env_from(
    agent_dir: &Path,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Option<Arc<extension::FluxExtension>> {
    Some(flux_extension_with_root(
        resources::BundledRoot::resolve_from(agent_dir, env),
    ))
}
