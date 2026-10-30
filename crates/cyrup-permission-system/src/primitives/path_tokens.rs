//! Safe path-token validation (R-SA-087) — vendored from the deleted `cyrup-ext-subagents` crate
//! (docs/LEAN-FORK-PLAN.md step 4), out of its `background/control.rs`, behaviour unchanged.
//! `forwarding.rs` gates every externally supplied id before it takes part in a `Path::join`, and
//! maps [`PathTokenError`] onto its own `PermissionError::UnsafeToken`.

use std::path::Path;

/// Why a token or resolved path was refused — this crate maps it onto
/// `PermissionError::UnsafeToken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathTokenError(pub String);

impl std::fmt::Display for PathTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PathTokenError {}

/// Validates `token` as a "safe token" (R-SA-087): non-empty, no path separators (`/` or `\`),
/// and no `..` component — before it is used to construct ANY filesystem path. This MUST be
/// called on every externally supplied run-id/step-index/profile-name-shaped string before it
/// participates in a [`Path::join`] anywhere in this module.
///
/// This is a pure string check — it does not touch the filesystem. Verifying that an already-
/// constructed path stays inside its configured root is [`validate_contains_root`]'s separate,
/// complementary job (R-SA-087's second clause).
///
/// # Errors
///
/// Returns [`PathTokenError`] if `token` is empty or contains a path separator or
/// a `..` component.
pub fn validate_safe_token(token: &str) -> Result<(), PathTokenError> {
    if token.is_empty() {
        return Err(PathTokenError(
            "token must not be empty".to_string(),
        ));
    }
    if token.contains('/') || token.contains('\\') {
        return Err(PathTokenError(format!(
            "token must not contain a path separator: {token:?}"
        )));
    }
    if token.contains("..") {
        return Err(PathTokenError(format!(
            "token must not contain \"..\": {token:?}"
        )));
    }
    Ok(())
}

/// Verifies that `resolved` (a path already constructed from a validated token) lies strictly
/// inside `root` (R-SA-087's second clause: "resolved paths MUST be verified to remain inside the
/// configured async/results roots"). This is a lexical containment check (`starts_with` on the
/// path components), not a symlink-resolving filesystem check — both `root` and `resolved` in
/// every call site in this module are constructed by plain [`Path::join`] from an already-
/// safe-token-validated component, so a lexical check is sufficient defense-in-depth without
/// requiring `resolved` to already exist on disk (which it frequently does not yet, e.g. a
/// not-yet-created `RunDir`).
///
/// # Errors
///
/// Returns [`PathTokenError`] if `resolved` does not lie inside `root`.
pub fn validate_contains_root(root: &Path, resolved: &Path) -> Result<(), PathTokenError> {
    if resolved.starts_with(root) {
        Ok(())
    } else {
        Err(PathTokenError(format!(
            "resolved path {resolved:?} escapes configured root {root:?}"
        )))
    }
}
