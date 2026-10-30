//! The binaries and the wasm component this suite spawns, by absolute path.
//!
//! THE RULE THIS EXISTS TO WORK AROUND: `CARGO_BIN_EXE_<name>` is set only for test targets in the
//! **same package** as that binary. It does not cross workspace members, so
//! `env!("CARGO_BIN_EXE_cyrup")` — the form used by 51 files before the migration, and documented
//! in-repo at `crates/cyrup-ext-subagents/tests/background_runner_main_integration.rs:103` — does
//! not compile here. `build.rs` resolves the paths once and re-exports them as `CYRUP_IT_BIN_*`;
//! these accessors are the only place that spells those names.
//!
//! Migration rewrite (mechanical, assertion-preserving):
//!
//! ```ignore
//! - Command::new(env!("CARGO_BIN_EXE_cyrup"))
//! + Command::new(support::bins::cyrup())
//! ```
//!
//! `env!` resolves at compile time, so a missing variable is a build error naming the target, not
//! a mysterious runtime `No such file or directory`.

use std::path::{Path, PathBuf};

/// The real `cyrup` binary (`crates/cyrup`).
pub fn cyrup() -> PathBuf {
    PathBuf::from(env!("CYRUP_IT_BIN_CYRUP"))
}

/// The standalone intercom broker (`crates/cyrup-intercom/src/bin/cyrup-intercom-broker.rs`).
pub fn intercom_broker() -> PathBuf {
    PathBuf::from(env!("CYRUP_IT_BIN_CYRUP_INTERCOM_BROKER"))
}

/// The intercom scripting client (`crates/cyrup-intercom/src/bin/cyrup-intercom-cli.rs`, the port
/// of pi-intercom's `cli.ts`).
pub fn intercom_cli() -> PathBuf {
    PathBuf::from(env!("CYRUP_IT_BIN_CYRUP_INTERCOM_CLI"))
}

/// The `wasm32-wasip2` guest component, built once by `build.rs` for the whole suite.
///
/// Replaces 22 byte-identical `fixture_component()` helpers, 24 nested `cargo build` invocations,
/// and two never-cleaned fixed `$TMPDIR` target dirs. `CYRUP_EXT_FIXTURE_COMPONENT` still
/// overrides it — now at one place instead of 22.
pub fn component() -> PathBuf {
    PathBuf::from(env!("CYRUP_IT_COMPONENT"))
}

/// The component's bytes, which is what `ExtensionHost::load_wasm` actually wants.
pub fn component_bytes() -> Vec<u8> {
    let path = component();
    read(&path)
}

fn read(path: &Path) -> Vec<u8> {
    match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => panic!("read wasm component at {}: {e}", path.display()),
    }
}
