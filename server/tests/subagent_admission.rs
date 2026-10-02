//! Exercises subagent admission through the Cursor HTTP and transport boundary.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use cursor_server::{
    api::cursor,
    cursor::{
        prompting::{PromptAssets, PromptCompiler},
        protocol::{
            connect,
            proto::{agent::v1 as pb, aiserver::v1 as ai},
        },
        TransportCommand, TransportRegistry,
    },
    network::NetworkClients,
    provider::{FinishReason, ModelEvent},
    store::{BlobId, Store},
};
use prost::Message;
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

fn registry(store: &Store, provider: &fake_provider::FakeProvider) -> TransportRegistry {
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    TransportRegistry::new(
        store.clone(),
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    )
}

fn run(message_id: &str, state: Option<pb::ConversationStateStructure>) -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                conversation_id: Some("subagent-admission-conversation".into()),
                subagent_type_name: Some("generalPurpose".into()),
                requested_model: Some(pb::RequestedModel {
                    model_id: "plugin:test/provider/model".into(),
                    ..Default::default()
                }),
                conversation_state: state,
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::UserMessageAction(
                        pb::UserMessageAction {
                            user_message: Some(pb::UserMessage {
                                text: "review this file again".into(),
                                message_id: message_id.into(),
                                mode: pb::AgentMode::Agent as i32,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
    }
}

async fn append(
    registry: &TransportRegistry,
    request_id: &str,
    headers: &[(&str, &str)],
    message: pb::AgentClientMessage,
) -> axum::response::Response {
    let wire = ai::BidiAppendRequest {
        request_id: Some(ai::BidiRequestId {
            request_id: request_id.into(),
        }),
        data: hex::encode(message.encode_to_vec()),
        ..Default::default()
    };
    let mut request = Request::post("/aiserver.v1.BidiService/BidiAppend")
        .header("content-type", "application/proto");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    cursor::router(
        registry.clone(),
        NetworkClients::new(registry.store().clone()),
    )
    .unwrap()
    .oneshot(request.body(Body::from(wire.encode_to_vec())).unwrap())
    .await
    .unwrap()
}

async fn complete(
    registry: &TransportRegistry,
    provider: &fake_provider::FakeProvider,
    request_id: &str,
    headers: &[(&str, &str)],
    message: pb::AgentClientMessage,
) -> pb::ConversationStateStructure {
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: request_id.into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("review done".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let handle = registry.get_or_create(request_id).await.unwrap();
    let mut output = handle.subscribe();
    let response = append(registry, request_id, headers, message).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let mut state = None;
    let mut seqno = 1;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .expect("terminal frame");
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            let terminal: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            assert!(terminal.get("error").is_none(), "{terminal}");
            break;
        }
        match pb::AgentServerMessage::decode(payload).unwrap().message {
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(checkpoint)) => {
                state = Some(checkpoint)
            }
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                assert!(matches!(
                    kv.message,
                    Some(pb::kv_server_message::Message::SetBlobArgs(_))
                ));
                handle
                    .command(TransportCommand::Append {
                        seqno,
                        message: Box::new(pb::AgentClientMessage {
                            message: Some(pb::agent_client_message::Message::KvClientMessage(
                                pb::KvClientMessage {
                                    id: kv.id,
                                    message: Some(pb::kv_client_message::Message::SetBlobResult(
                                        pb::SetBlobResult { error: None },
                                    )),
                                },
                            )),
                        }),
                    })
                    .await
                    .unwrap();
                seqno += 1;
            }
            _ => {}
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let active: Option<String> = sqlx::query_scalar("SELECT active_run_id FROM conversations WHERE conversation_id = 'subagent-admission-conversation'").fetch_one(registry.store().pool()).await.unwrap();
            if active.is_none() { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    state.expect("checkpoint emitted")
}

#[tokio::test]
async fn subagent_starts_with_only_parent_request_header() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    complete(
        &registry,
        &provider,
        "request-only",
        &[("x-parent-request-id", "parent")],
        run("user", None),
    )
    .await;
    assert_eq!(provider.requests().len(), 1);
    registry.shutdown().await;
}

#[tokio::test]
async fn subagent_starts_with_only_parent_tool_header() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    complete(
        &registry,
        &provider,
        "tool-only",
        &[("x-parent-agent-tool-call-id", "task")],
        run("user", None),
    )
    .await;
    assert_eq!(provider.requests().len(), 1);
    registry.shutdown().await;
}

#[tokio::test]
async fn subagent_starts_with_paired_or_absent_headers() {
    for headers in [
        vec![],
        vec![
            ("x-parent-request-id", "parent"),
            ("x-parent-agent-tool-call-id", "task"),
        ],
    ] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = fake_provider::FakeProvider::default();
        let registry = registry(&store, &provider);
        complete(
            &registry,
            &provider,
            "paired-or-absent",
            &headers,
            run("user", None),
        )
        .await;
        assert_eq!(provider.requests().len(), 1);
        registry.shutdown().await;
    }
}

#[tokio::test]
async fn idless_subagent_retries_deduplicate_but_repeated_turns_keep_history() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    let first = complete(&registry, &provider, "idless-first", &[], run("", None)).await;
    complete(&registry, &provider, "idless-retry", &[], run("", None)).await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE message_id LIKE 'runtime:%'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(count, 1, "retry must not append a second user event");
    assert_eq!(
        provider.requests()[0].history,
        provider.requests()[1].history
    );
    // Start a new transport registry to exercise checkpoint hydration after restart.
    registry.shutdown().await;
    let registry = self::registry(&store, &provider);
    let second = complete(
        &registry,
        &provider,
        "idless-next-turn",
        &[],
        run("", Some(first.clone())),
    )
    .await;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE message_id LIKE 'runtime:%'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(
        count, 2,
        "the same words after a completed turn are a new input"
    );
    let requests = provider.requests();
    assert_eq!(
        requests[0].history,
        requests[2].history[..requests[0].history.len()]
    );
    assert!(requests[2].history.len() > requests[0].history.len());
    assert_eq!(second.turns.len(), first.turns.len() + 1);
    let turn = pb::ConversationTurnStructure::decode(
        store
            .get_blob(&BlobId::from_bytes(second.turns.last().unwrap()).unwrap())
            .await
            .unwrap()
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    let Some(pb::conversation_turn_structure::Turn::AgentConversationTurn(turn)) = turn.turn else {
        panic!("agent turn")
    };
    let user = pb::UserMessage::decode(
        store
            .get_blob(&BlobId::from_bytes(&turn.user_message).unwrap())
            .await
            .unwrap()
            .unwrap()
            .as_slice(),
    )
    .unwrap();
    assert!(!user.message_id.is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn empty_parent_headers_are_rejected_before_provider_execution() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    for (index, headers) in [
        vec![("x-parent-request-id", "")],
        vec![("x-parent-agent-tool-call-id", "")],
        vec![
            ("x-parent-request-id", "parent"),
            ("x-parent-agent-tool-call-id", ""),
        ],
    ]
    .iter()
    .enumerate()
    {
        let response = append(
            &registry,
            &format!("empty-{index}"),
            headers,
            run("user", None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn parent_metadata_enrichment_is_atomic_and_never_clears_known_ids() {
    use cursor_server::cursor::TransportParent;
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    let handle = registry.get_or_create("metadata").await.unwrap();
    let request_only = TransportParent {
        request_id: Some("parent".into()),
        tool_call_id: None,
    };
    let tool_only = TransportParent {
        request_id: None,
        tool_call_id: Some("task".into()),
    };
    handle.set_parent(request_only.clone()).unwrap();
    // A conflicting request ID must not partially add the new tool ID.
    assert!(handle
        .set_parent(TransportParent {
            request_id: Some("other".into()),
            tool_call_id: Some("wrong-task".into())
        })
        .is_err());
    assert_eq!(handle.parent(), Some(request_only.clone()));
    handle.set_parent(tool_only.clone()).unwrap();
    handle.set_parent(request_only).unwrap();
    handle.set_parent(tool_only).unwrap();
    let expected = handle.parent();
    assert_eq!(
        expected,
        Some(TransportParent {
            request_id: Some("parent".into()),
            tool_call_id: Some("task".into())
        })
    );
    assert!(handle
        .set_parent(TransportParent {
            request_id: None,
            tool_call_id: Some("other-task".into())
        })
        .is_err());
    assert_eq!(handle.parent(), expected);
    registry.shutdown().await;
}

#[tokio::test]
async fn missing_message_id_on_a_root_run_is_still_rejected() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    let handle = registry.get_or_create("invalid-root").await.unwrap();
    let mut output = handle.subscribe();
    let mut message = run("", None);
    let Some(pb::agent_client_message::Message::RunRequest(request)) = message.message.as_mut()
    else {
        unreachable!()
    };
    request.subagent_type_name = None;
    assert_eq!(
        append(&registry, "invalid-root", &[], message)
            .await
            .status(),
        StatusCode::OK
    );
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            let error: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            assert!(error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("has no message_id"));
            break;
        }
    }
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn idless_subagent_with_only_tool_header_keeps_parent_progress() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "progress".into(),
        },
        ModelEvent::ToolCallStart {
            index: 0,
            call_id: "progress-call".into(),
            name: "UpdateCurrentStep".into(),
        },
        ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: r#"{"current_step":"reviewing","final_summary":"review complete"}"#.into(),
        },
        ModelEvent::ToolCallEnd { index: 0 },
        ModelEvent::Done(FinishReason::ToolUse),
    ]);
    let registry = registry(&store, &provider);
    let state = complete(
        &registry,
        &provider,
        "idless-progress",
        &[("x-parent-agent-tool-call-id", "parent-task")],
        run("", None),
    )
    .await;
    let progress = state
        .communicate_update_states_by_parent_tool_call_id
        .get("parent-task")
        .expect("parent association survives partial headers");
    assert_eq!(progress.final_summary.as_deref(), Some("review complete"));
    assert_eq!(provider.requests().len(), 2);
    registry.shutdown().await;
}
