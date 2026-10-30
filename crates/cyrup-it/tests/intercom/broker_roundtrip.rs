//! FULLY-WIRED PROOF (the port doc §5 Phase 2/§11.1): stand up the **real** broker as a genuine
//! child OS process (the `cyrup-intercom-broker` fixture binary) and drive an ask/answer round trip
//! between two sessions through **two real `IntercomClient`s over the real Unix socket** — not a
//! mock (the port doc §7.4).
//!
//! [CYRUP-DELTA] docs/LEAN-FORK-PLAN.md step 4 deleted pi's subagents extension, and with it the
//! third test that used to sit here — `delivery_channel_relays_a_grouped_result_to_the_supervisor…`,
//! which drove the `DeliveryChannel` seam a subagent orchestrator used to hand its grouped run result
//! to its parent. The seam itself is gone, so the proof went with it. What stays is the part that
//! does not depend on a subagent: the broker's own message, reply-edge and mutual-ask behavior.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::Duration;

use crate::common::registration;
use cyrup_intercom::transport::client::{InboundEvent, IntercomClient, SendOptions};
use cyrup_intercom::transport::spawn::wait_for_broker;

/// Wait for the next `Message` inbound event on `rx`, with a bound.
async fn next_message(
    rx: &mut tokio::sync::broadcast::Receiver<InboundEvent>,
) -> (
    cyrup_intercom::transport::protocol::SessionInfo,
    cyrup_intercom::transport::protocol::Message,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(InboundEvent::Message { from, message })) => return (from, *message),
            Ok(Ok(_other)) => continue,
            Ok(Err(e)) => panic!("event channel error: {e}"),
            Err(_) => panic!("timed out waiting for an inbound message"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_to_broker_to_supervisor_round_trip_over_the_real_socket() {
    let broker_bin = crate::support::bins::intercom_broker();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = agent_dir.path().join("intercom");
    let socket_path = intercom_dir.join("broker.sock");

    // Launch the REAL broker as a genuine child process, pointing it at our temp agent dir.
    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn the real intercom broker subprocess");

    // Wait for the broker's health probe to answer over the real socket.
    wait_for_broker(&socket_path, Duration::from_secs(5))
        .await
        .expect("broker becomes health-connectable");

    // Supervisor connects with a STABLE session id (so the child can address it deterministically).
    let supervisor = IntercomClient::connect(
        &socket_path,
        registration("supervisor"),
        Some("supervisor-session".to_string()),
    )
    .await
    .expect("supervisor registers");
    assert_eq!(
        supervisor.session_id().as_deref(),
        Some("supervisor-session")
    );
    let mut supervisor_events = supervisor.subscribe();

    // Child connects.
    let child = IntercomClient::connect(&socket_path, registration("subagent-chat-1"), None)
        .await
        .expect("child registers");
    let child_id = child.session_id().expect("child has a session id");

    // 1) Child asks the supervisor (expects_reply → records an ask edge on the broker).
    let question_id = uuid::Uuid::new_v4().to_string();
    let send_result = child
        .send(
            "supervisor-session",
            SendOptions {
                text: "Which database should I use?".to_string(),
                expects_reply: Some(true),
                message_id: Some(question_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("child send routes through the broker");
    assert!(
        send_result.delivered,
        "the broker delivered the ask to the supervisor: {send_result:?}"
    );

    // 2) Supervisor RECEIVES the ask, child→broker→supervisor.
    let (from, ask) = next_message(&mut supervisor_events).await;
    assert_eq!(from.id, child_id, "the ask came from the child session");
    assert_eq!(ask.id, question_id);
    assert_eq!(ask.expects_reply, Some(true));
    assert_eq!(ask.content.text, "Which database should I use?");

    // 3) Supervisor replies (reply_to = questionId), routed back over the SAME broker.
    let mut child_events = child.subscribe();
    let reply_result = supervisor
        .send(
            &child_id,
            SendOptions {
                text: "Use Postgres.".to_string(),
                reply_to: Some(question_id.clone()),
                ..Default::default()
            },
        )
        .await
        .expect("supervisor reply routes through the broker");
    assert!(
        reply_result.delivered,
        "the reply was delivered to the still-connected child"
    );

    // 4) The child RECEIVES the answer — the full round trip closes.
    let (reply_from, reply) = next_message(&mut child_events).await;
    assert_eq!(reply_from.id, "supervisor-session");
    assert_eq!(reply.reply_to.as_deref(), Some(question_id.as_str()));
    assert_eq!(reply.content.text, "Use Postgres.");

    // Also prove list discovery sees both sessions through the real broker.
    let sessions = supervisor.list_sessions().await.expect("list sessions");
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&"supervisor-session"));
    assert!(ids.contains(&child_id.as_str()));

    // Clean teardown.
    child.disconnect();
    supervisor.disconnect();
    let _ = broker.kill().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_refuses_a_mutual_ask() {
    let broker_bin = crate::support::bins::intercom_broker();
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let socket_path = agent_dir.path().join("intercom").join("broker.sock");

    let mut broker = tokio::process::Command::new(&broker_bin)
        .env("CYRUP_CODING_AGENT_DIR", agent_dir.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn broker");
    wait_for_broker(&socket_path, Duration::from_secs(5))
        .await
        .expect("broker up");

    let a = IntercomClient::connect(&socket_path, registration("a"), Some("sess-a".to_string()))
        .await
        .expect("a registers");
    let b = IntercomClient::connect(&socket_path, registration("b"), Some("sess-b".to_string()))
        .await
        .expect("b registers");

    // a asks b (records edge a→b).
    let q1 = uuid::Uuid::new_v4().to_string();
    let r1 = a
        .send(
            "sess-b",
            SendOptions {
                text: "q".to_string(),
                expects_reply: Some(true),
                message_id: Some(q1),
                ..Default::default()
            },
        )
        .await
        .expect("a→b ask");
    assert!(r1.delivered);

    // b tries to ask a back while a's ask is open → mutual-ask refusal (broker.ts:460-469).
    let q2 = uuid::Uuid::new_v4().to_string();
    let r2 = b
        .send(
            "sess-a",
            SendOptions {
                text: "q2".to_string(),
                expects_reply: Some(true),
                message_id: Some(q2),
                ..Default::default()
            },
        )
        .await
        .expect("b→a ask returns a broker verdict");
    assert!(!r2.delivered, "a mutual ask must be refused");
    assert!(
        r2.reason
            .as_deref()
            .unwrap_or_default()
            .contains("Mutual ask refused"),
        "reason: {:?}",
        r2.reason
    );

    a.disconnect();
    b.disconnect();
    let _ = broker.kill().await;
}
