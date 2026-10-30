//! Argv routing that must run **before** clap ever sees the command line.
//!
//! Five gates run ahead of `parseArgs`, in pi's own order, and each can end the process:
//!
//! 1. the internal `__intercom-broker` hop (spec/extensions/cyrup-intercom-port.md §7.3),
//! 2. the internal `__mcp-keyring-helper` hop (13f-mcp-credentials MCP-260/MCP-261),
//! 3. the package/config subcommands (pi `handlePackageCommand`, main.ts:486),
//! 4. `auth print-api-key|print-bearer-token` (pi `runCredentialPrintCommand`, main.ts:557-559).
//!
//! **[CYRUP-DELTA]** pi's two `__subagent-*` hops (docs/LEAN-FORK-PLAN.md step 4) are gone with the
//! subagents extension, so the list here is shorter than pi's.
//!
//! The internal hops are **classified** here and **dispatched** by `main.rs`, rather than
//! being dispatched here, for one specific reason: each one first re-labels the process
//! (`cyrup-broker` / `cyrup-mcp-keyring`, SEAM-070) and
//! `set_process_name` needs `unsafe` (`prctl(PR_SET_NAME)` / `pthread_setname_np`), which this
//! crate's `#![forbid(unsafe_code)]` rules out. Splitting classification from dispatch keeps the
//! `unsafe` in the binary — where the rest of the process-identity work already lives — without
//! threading a callback through this module.
//!
//! **A classification arm with no matching `main.rs` dispatch arm fails SILENTLY**: the argv falls
//! through to clap, which rejects the hop's own flags with a usage error and exit 2. That is why
//! each hop is covered end to end by a test that spawns the REAL binary (see
//! `crates/cyrup/tests/` and the `bin` target of `cyrup-it`).

use anyhow::Context;
use cyrup_config::{CliConfigOverrides, ConfigDirs, EnvVars};

use crate::{
    acp_terminal_login_cmd, credential_print, intercom_broker_cmd, mcp_keyring_helper_cmd,
    subcommands,
};

/// Which internal, never-advertised subcommand this argv selects, if any.
///
/// None of these appears in `--help` or in `subcommands::SUBCOMMANDS`. All of them MUST be
/// recognized before ANY user-facing arg leniency/clap parsing — and before the package/config
/// pre-dispatch, which has no knowledge of them and would otherwise fall through to ordinary clap
/// parsing, misinterpreting `--config <path>` against the user-facing `Cli` surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Internal {
    /// `__intercom-broker` — the hidden subcommand the per-session intercom extension re-execs
    /// `current_exe()` into to stand up the standalone broker PROCESS (a Unix-socket hub). Its
    /// `--config`-free argv must never reach the user-facing `Cli` surface.
    IntercomBroker,
    /// `__mcp-keyring-helper` — the hidden subcommand
    /// [`cyrup_mcp::credentials::McpAuthStore`]'s Linux keyring-recovery arm re-execs
    /// `current_exe()` into, as `<keyctl> session - <cyrup> __mcp-keyring-helper`, when the session
    /// keyring has been revoked and an MCP OAuth credential read/write/remove must be retried
    /// inside a fresh anonymous session keyring (MCP-260). The child speaks one line of JSON on
    /// stdin and one on stdout, so it must reach [`crate::mcp_keyring_helper_cmd::dispatch`] before
    /// anything can log, print, or otherwise put a byte on stdout.
    McpKeyringHelper,
    /// `--terminal-login` — the ACP client's Authenticate button (ACP-001). Port of pi-acp v0.0.33
    /// `index.ts`'s top-level `process.argv.includes("--terminal-login")` block.
    ///
    /// **The odd one out in three ways, and each is load-bearing.** (1) Its predicate is membership
    /// ANYWHERE after the program name, not `argv[1]`, because an ACP client appends
    /// `AuthMethod.args` to a command it already holds — see
    /// [`crate::acp_terminal_login_cmd::is_selected`]. (2) It is not `__`-prefixed and is not
    /// internal: it is a token cyrup publishes to clients in `AuthMethod::Terminal.args`
    /// (`ACP-011`). (3) It does **not** end the process — `main` strips its tokens and falls
    /// through into the ordinary interactive launch, which is why it is classified LAST (a hop's
    /// own argv could otherwise contain the token and hijack it).
    AcpTerminalLogin,
}

/// Classify the internal pre-dispatch. `raw` (not the program-stripped `argv`) is passed because
/// every `is_selected` predicate expects the binary name at index 0, matching
/// `std::env::args()`'s own shape.
pub fn classify_internal(raw: &[String]) -> Option<Internal> {
    if intercom_broker_cmd::is_selected(raw) {
        return Some(Internal::IntercomBroker);
    }
    if mcp_keyring_helper_cmd::is_selected(raw) {
        return Some(Internal::McpKeyringHelper);
    }
    // ACP-001 — LAST, because its predicate is membership anywhere in argv rather than a
    // fixed position: checking it first would let a `--terminal-login` appearing in one of the
    // internal subcommands' own argv hijack that hop.
    if acp_terminal_login_cmd::is_selected(raw) {
        return Some(Internal::AcpTerminalLogin);
    }
    None
}

/// The two user-facing pre-clap gates, in pi's order. `Some(code)` means the run was handled and
/// the caller returns that exit code.
///
/// The package/config subcommands resolve their dirs with NO CLI overrides, for the subcommand's
/// own package/project roots (pi main.ts:486, before arg parsing) — and only when a subcommand is
/// actually present, so an ordinary launch never pays for (or fails on) a resolve it will redo with
/// the real overrides.
///
/// The credential-print gate is pi's `if (await runCredentialPrintCommand(args)) return;`
/// (main.ts:557-559) — immediately after the config/package block and BEFORE `parseArgs`. Without
/// it `auth` is not a known verb, so the tokens survive arg leniency as bare positionals and become
/// a chat PROMPT: no credential, no error, an agent session started and tokens burned on an auth
/// subcommand.
pub async fn run_predispatch(argv: &[String]) -> anyhow::Result<Option<i32>> {
    if subcommands::first_subcommand(argv).is_some() {
        let env = EnvVars::from_process();
        let dirs = ConfigDirs::resolve(&CliConfigOverrides::default(), &env)
            .context("resolving config directories")?;
        let trust_override = subcommands::trust_override(argv);
        if let Some(code) = subcommands::dispatch(argv, &dirs, trust_override).await? {
            return Ok(Some(code));
        }
    }
    if let Some(code) = credential_print::dispatch(argv).await {
        return Ok(Some(code));
    }
    Ok(None)
}
