//! **A hung herdr must not eat the `SessionShutdown` handler's dispatch budget.**
//!
//! This moved out of `cyrup-ext-subagents` with the release itself: arming, feeding and releasing
//! the pane are one owner's job — `cyrup_ext_herdr`'s own built-in — and the subagents extension's
//! `SessionShutdown` arm no longer waits on herdr at all. The hazard the test pins is unchanged,
//! and it is a general one:
//!
//! [`cyrup_ext_herdr::extension`]`'s `SessionShutdown` arm runs under the dispatcher's per-handler
//! invocation budget ([`cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET`], 5 s), and the dispatcher
//! enforces that budget by **dropping the handler future** (`Dispatcher::invoke_contained`'s
//! `tokio::time::timeout`). `cyrup_ext_herdr::shutdown()` is bounded by
//! [`cyrup_ext_herdr::reporter::RELEASE_TIMEOUT`] — also 5 s. So a plain
//! `shutdown().await` as the arm's first statement means a herdr that accepted the connection and
//! then answered nothing consumes the ENTIRE budget on the one statement the arm exists for, and
//! the arm is dropped mid-release rather than returning.
//!
//! The arm therefore SPAWNS the release and joins with a smaller, `const`-asserted slice
//! (`HERDR_RELEASE_JOIN_BUDGET`, 2 s): the release goes out first, the waiting is bounded, and a
//! timed-out join leaves the task draining on the runtime until `RELEASE_TIMEOUT` ends it.
//!
//! Driven for real: a `UnixListener` that accepts and answers nothing (the only way to make the
//! release hang with no herdr installed), `arm_in` parking a real bridge in the process-global
//! slot, and a real [`cyrup_ext::Dispatcher`] at its real budget holding the built-in through a
//! real `NativeHandle` — so the truncation, if it happened, would be the dispatcher's own.
//!
//! The global bridge slot is process-wide, which is safe here because `nextest` runs each test in
//! its own process.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;
use cyrup_ext::event::{EventKind, Subscriptions};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension, NativeHandle};
use cyrup_ext::{Dispatcher, HostEvent};

/// A fake herdr that accepts connections and never writes a byte.
///
/// This is the failure mode `reporter::RELEASE_TIMEOUT` exists for and the one the socket's own
/// per-call deadline (`cyrup_herdr::DEFAULT_TIMEOUT`, 15 s) cannot shorten: the connection is
/// established, so nothing errors — the release simply waits.
struct BlackHoleHerdr {
    path: std::path::PathBuf,
    accept: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for BlackHoleHerdr {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl BlackHoleHerdr {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bind the black-hole socket");
        let accept = tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let Ok((stream, _addr)) = listener.accept().await else {
                    return;
                };
                // Held, never read, never answered.
                held.push(stream);
            }
        });
        Self {
            path,
            accept,
            _dir: dir,
        }
    }

    /// The environment herdr injects into a pane it owns, pointed at this socket.
    fn pane_env(&self) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::from([
            ("HERDR_ENV".to_owned(), "1".to_owned()),
            ("HERDR_PANE_ID".to_owned(), "w1:p1".to_owned()),
            (
                "HERDR_SOCKET_PATH".to_owned(),
                self.path.display().to_string(),
            ),
            ("HERDR_TAB_ID".to_owned(), "w1:t1".to_owned()),
        ])
    }
}

/// **The built-in's release arm returns inside the dispatch budget, and the slot still clears.**
///
/// *Gutted by*: putting `cyrup_ext_herdr::shutdown().await` back as the arm's first statement (the
/// dispatch then burns the full budget on it and the handler is dropped at the deadline rather
/// than returning); raising `HERDR_RELEASE_JOIN_BUDGET` to the dispatch budget or beyond — which
/// the `const` assertion beside that constant refuses at compile time, the point of it being a
/// `const`; deleting the join's `tokio::time::timeout` so the arm waits out the full
/// `RELEASE_TIMEOUT` and is dropped anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_hung_herdr_cannot_run_the_session_shutdown_handler_over_its_budget() {
    let herdr = BlackHoleHerdr::start();

    // `SessionStart`'s arm, with the environment as a parameter (`arm_in`'s own doc).
    let armed = cyrup_ext_herdr::arm_in(&herdr.pane_env(), true, None);
    assert!(
        armed.is_some(),
        "a pane env with a UI arms the bridge; without one this test proves nothing"
    );
    assert!(
        cyrup_ext_herdr::bridge().is_some(),
        "armed into the global slot"
    );

    // The real dispatcher, at its real budget, holding the real built-in the real way.
    let dispatcher = Dispatcher::new();
    let ctx = HostCtx::event(
        ExtMode::Tui,
        true,
        std::env::current_dir().unwrap_or_default(),
    );
    dispatcher
        .add(Arc::new(NativeHandle::new(
            Arc::new(cyrup_ext_herdr::extension::HerdrStatusExtension::new())
                as Arc<dyn NativeExtension>,
            Subscriptions::empty().with(EventKind::SessionShutdown),
            ctx,
        )))
        .expect("the dispatcher takes the built-in");

    let started = Instant::now();
    dispatcher
        .dispatch_notify(
            &HostEvent::SessionShutdown {
                reason: "quit".to_owned(),
                target_session_file: None,
            },
            &CancelToken::new(),
        )
        .await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < cyrup_ext::dispatch::DEFAULT_INVOKE_BUDGET,
        "the handler must RETURN inside the dispatch budget rather than be dropped at it; \
         took {elapsed:?}"
    );
    // The bridge is out of the global slot even though its release never completed — `shutdown()`
    // TAKES the slot before it awaits anything, so a rebuilt session cannot report through it.
    assert!(
        cyrup_ext_herdr::bridge().is_none(),
        "shutdown must clear the process-global slot"
    );
    // And the release really was still in flight: a socket that never answers cannot have
    // completed inside the join budget, so the join — not a clean return — is what bounded this.
    assert!(
        elapsed >= Duration::from_secs(1),
        "the join budget was not actually exercised; the release must still have been pending"
    );
}
