//! `machine_hostname` — vendored from the deleted `cyrup-ext-subagents` crate
//! (docs/LEAN-FORK-PLAN.md step 4), out of its `background/async_retention/lock.rs`, unchanged.
//! Rung 1 of the "is the forwarding owner gone?" ladder compares this against the hostname stamped
//! on the owner record, so it MUST keep answering the way the stamping side answered.

/// This crate is `#![forbid(unsafe_code)]`, so `libc::gethostname` (a raw `*mut c_char` out-param)
/// is not available to it and no workspace crate wraps one. `/proc/sys/kernel/hostname` is the
/// kernel's own answer on Linux — the same string `gethostname(2)` returns, since both read
/// `uts_namespace->name.nodename` — and `/etc/hostname` is the portable-Unix fallback. `HOSTNAME`
/// is consulted last because a shell variable can be stale or absent.
///
/// A machine whose hostname cannot be determined gets the literal `"unknown-host"`. That is the
/// SAFE direction and not a degradation: rung 2 of the ladder compares the owner's hostname with
/// this one, so two processes that both fail to resolve it agree with each other (they really are
/// on the same machine), and a process that resolves it will never break a lock stamped
/// `"unknown-host"` — it will wait out [`LOCK_STALE_MS`] instead.
#[must_use]
pub fn machine_hostname() -> String {
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(raw) = std::fs::read_to_string(path) {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    for key in ["HOSTNAME", "COMPUTERNAME"] {
        if let Ok(value) = std::env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    "unknown-host".to_string()
}
