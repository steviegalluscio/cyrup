//! Process-liveness probes — vendored from the deleted `cyrup-ext-subagents` crate
//! (docs/LEAN-FORK-PLAN.md step 4), taken unchanged in behaviour out of its
//! `background/reconcile.rs`.
//!
//! `forwarding.rs` needs exactly these: the three-outcome [`Liveness`] classification, the
//! zero-signal probe, the zombie probe and the PID-namespace scope rule — the permission system's
//! `no-ui` marker declares a forwarding parent gone by the same ladder the subagent reconciler
//! used, and it must stay one implementation. pi citations are the ones the original carried.

/// The three-outcome classification a zero-signal liveness probe (`kill(pid, 0)` on Unix) MUST
/// distinguish (R-SA-089). **Never collapse [`Liveness::Unknown`] into [`Liveness::Dead`]** — an
/// `EPERM`-class probe failure commonly means the process exists but liveness cannot be confirmed
/// under sandboxing, which is a materially different situation from "no such process".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The probe succeeded: the process exists (and this process has permission to signal it).
    Alive,
    /// The probe failed with "no such process" (`ESRCH`): the pid is confirmed gone.
    Dead,
    /// The probe failed with anything other than `ESRCH` (most commonly `EPERM`/permission-denied
    /// under sandboxing) — the process may well still exist; this check simply cannot confirm
    /// either way.
    Unknown,
}

impl Liveness {
    /// `true` for [`Liveness::Alive`] or [`Liveness::Unknown`] — the two outcomes that do NOT, on
    /// their own, prove the process is gone (R-SA-089's "Unknown MUST NOT be treated as dead").
    /// Reconciliation's staleness check (step 4 of the algorithm) applies identically to both.
    #[must_use]
    pub fn is_possibly_alive(self) -> bool {
        matches!(self, Liveness::Alive | Liveness::Unknown)
    }
}

/// A real zero-signal liveness probe against `pid` (R-SA-089): `kill(pid, 0)` sends no actual
/// signal — the kernel performs only existence/permission checks — so this never disturbs a live
/// process, it only observes it.
///
/// - `Ok(())` → [`Liveness::Alive`]: the pid exists and this process may signal it.
/// - `Err(Errno::ESRCH)` → [`Liveness::Dead`]: the kernel has confirmed no such process exists.
/// - `Err(_)` (most commonly `Errno::EPERM` under sandboxing, or any other unexpected errno) →
///   [`Liveness::Unknown`]: the probe is inconclusive, never treated as death.
///
/// On non-Unix targets, `nix::sys::signal::kill` is unavailable; conservatively reports
/// [`Liveness::Unknown`] for every pid rather than guessing, which routes every non-Unix
/// reconciliation decision through the same staleness-threshold path `Alive` uses, never the
/// immediate-failure `Dead` path.
#[must_use]
pub fn check_pid_liveness(pid: u32) -> Liveness {
    #[cfg(unix)]
    {
        // SIGNAL 0: existence/permission check only, no signal is actually delivered.
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
            Ok(()) => Liveness::Alive,
            Err(nix::errno::Errno::ESRCH) => Liveness::Dead,
            Err(_) => Liveness::Unknown,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Liveness::Unknown
    }
}
/// SUBA-159 — pi `currentPidNamespaceScope` (`runs/background/pid-namespace.ts:6-18` @ad11b7ab):
/// this process's Linux PID namespace identity, the `readlink("/proc/self/ns/pid")` target (for
/// example `pid:[4026531836]`), cached because reconciliation is a polling path. `None` off Linux,
/// and when the link cannot be read or is blank.
#[must_use]
pub fn current_pid_namespace_scope() -> Option<String> {
    static SCOPE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SCOPE
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            {
                std::fs::read_link("/proc/self/ns/pid")
                    .ok()
                    .and_then(|target| {
                        let scope = target.to_string_lossy().trim().to_string();
                        (!scope.is_empty()).then_some(scope)
                    })
            }
            #[cfg(not(target_os = "linux"))]
            {
                None
            }
        })
        .clone()
}

/// pi's `recorded !== undefined && recorded !== observed` over two PID namespace scopes
/// (`active-async-capacity.ts:256`, `await-async-run.ts:21-22` @ad11b7ab): a RECORDED scope the
/// observer does not share. An unrecorded scope is never a mismatch (a record an older build wrote
/// probes as before), and an observer with no scope of its own mismatches every recorded one. The
/// one expression of the rule; every caller that compares scopes goes through it.
#[must_use]
pub fn pid_namespace_scope_differs(recorded: Option<&str>, observed: Option<&str>) -> bool {
    recorded.is_some_and(|recorded| observed != Some(recorded))
}

/// pi `checkPidLiveness(pid, kill, probeZombie)` (`stale-run-reconciler.ts:360-379` @ad11b7ab):
/// [`check_pid_liveness`], plus — when `probe_zombie` is set and the probe found the pid — a read
/// of `/proc/<pid>/stat` whose state field (the character after the LAST `") "`, since the comm
/// field may itself contain one) reports a zombie as [`Liveness::Dead`]. A zombie answers
/// `kill(pid, 0)` but will never write another status, which is what a runner reparented to an
/// init that does not reap looks like.
///
/// Callers set `probe_zombie` only when the pid's recorded namespace equals this process's
/// (upstream's `pidScopeVerified`): a local `/proc/<pid>` says nothing about a pid from another
/// namespace. An unreadable `stat` falls back to the `kill(pid, 0)` answer, as upstream does.
#[must_use]
pub fn check_pid_liveness_probing_zombie(pid: u32, probe_zombie: bool) -> Liveness {
    let liveness = check_pid_liveness(pid);
    if liveness != Liveness::Alive || !probe_zombie {
        return liveness;
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            && stat_reports_zombie(&stat)
        {
            return Liveness::Dead;
        }
    }
    liveness
}

/// The state-field test of [`check_pid_liveness_probing_zombie`], split out so the parse is
/// testable against fixed text: pi `stat.lastIndexOf(") ")` then `stat[closeParen + 2] === "Z"`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn stat_reports_zombie(stat: &str) -> bool {
    stat.rfind(") ")
        .and_then(|close| stat.as_bytes().get(close + 2))
        .is_some_and(|state| *state == b'Z')
}

/// pi `processDemonstrablyGone` (`session-lease.ts:162-172` @v0.68.0) — "this pid is
/// demonstrably gone": DEAD, or ALIVE under a start identity that no longer matches the one
/// recorded, which is process-id reuse.
///
/// Both ambient probes are INJECTED, which is pi's own shape (`:165` takes `isProcessAlive` and
/// `getProcessStartIdentity` as required options); production callers pass
/// [`check_pid_liveness`] and
/// [`process_start_identity`](crate::primitives::session_lease::process_start_identity).
///
/// This is the single expression of the ladder, and
/// [`process_demonstrably_gone`](crate::primitives::session_lease::process_demonstrably_gone) is
/// this function compared against [`Liveness::Dead`]; neither re-states the rungs, so the lease's
/// staleness verdict and reconciliation's liveness verdict can never drift apart.
///
/// Generic over the two probes rather than taking `fn` pointers, because its three callers carry
/// them differently and none should have to re-state the ladder to fit: the session lease and the
/// retention lock hold plain `fn` pointers (which coerce), while
/// a capacity-options struct holds
/// `Arc<dyn Fn>` probes it hands over as closures.
#[must_use]
pub fn check_pid_identity_with(
    pid: u32,
    expected: Option<&crate::primitives::session_lease::ProcessStartIdentity>,
    liveness: impl FnOnce(u32) -> Liveness,
    start_identity_of: impl FnOnce(
        u32,
    )
        -> Option<crate::primitives::session_lease::ProcessStartIdentity>,
) -> Liveness {
    let observed = liveness(pid);
    // pi `:168` — `alive === false` is the only early `true`, and `:169`'s `alive !== true` folds
    // `Unknown` in with "no evidence".
    if observed != Liveness::Alive {
        return observed;
    }
    let Some(recorded) = expected else {
        return Liveness::Alive;
    };
    // pi `:171` — `currentIdentity !== undefined && currentIdentity !== startIdentity`.
    // \[CYRUP-DELTA] (PERM-045(b)) and only between tokens of one scheme: a `runtime:` token, or
    // one an older build's probe wrote, says nothing a `linux:` or `start:` reading can refute.
    match start_identity_of(pid) {
        Some(current) if current.scheme() == recorded.scheme() && current != *recorded => {
            Liveness::Dead
        }
        _ => Liveness::Alive,
    }
}
