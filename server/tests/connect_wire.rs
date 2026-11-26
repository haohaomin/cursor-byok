//! Verifies captured Cursor Connect framing and protobuf compatibility.
#[path = "support/fake_cursor.rs"]
mod fake_cursor;
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::{io::Write, sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use cursor_server::{
    api::cursor,
    cursor::prompting::{PromptAssets, PromptCompiler},
    cursor::protocol::{
        connect,
        proto::{agent::v1 as pb, aiserver::v1 as ai},
    },
    cursor::transport::TransportRegistry,
    network::NetworkClients,
    provider::{FinishReason, ModelEvent},
};
use flate2::{write::GzEncoder, Compression};
use prost::Message;
use tower::ServiceExt;

#[test]
fn connect_envelope_is_flag_plus_big_endian_length_plus_protobuf() {
    let message = pb::BidiRequestId {
        request_id: "abc".into(),
    };
    let frame = connect::encode_message(&message).unwrap();
    assert_eq!(&frame[..5], &[0, 0, 0, 0, 5]);
    let decoded: pb::BidiRequestId = fake_cursor::decode_single(&frame).unwrap();
    assert_eq!(decoded.request_id, "abc");
}

#[test]
fn end_stream_matches_captured_connect_shape() {
    assert_eq!(
        connect::encode_end_stream().as_ref(),
        &[2, 0, 0, 0, 2, b'{', b'}']
    );
}

#[test]
fn error_end_stream_is_flagged_json_not_protobuf() {
    let frame = connect::encode_error_end_stream(&connect::ConnectStreamError {
        code: connect::ConnectCode::Unavailable,
        message: "overloaded".into(),
        details: vec![connect::ConnectErrorDetail {
            type_name: "aiserver.v1.ErrorDetails".into(),
            value: "AQ".into(),
        }],
    })
    .unwrap();
    let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
    assert_eq!(flags, connect::END_STREAM_FLAG);
    let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(json["error"]["code"], "unavailable");
    assert_eq!(json["error"]["message"], "overloaded");
    assert_eq!(
        json["error"]["details"][0]["type"],
        "aiserver.v1.ErrorDetails"
    );
}

#[test]
fn cursor_error_details_subset_decodes_captured_wire_value() {
    let captured = "CAISVQoUQXV0aGVudGljYXRpb24gZXJyb3ISMklmIHlvdSBhcmUgbG9nZ2VkIGluLCB0cnkgbG9nZ2luZyBvdXQgYW5kIGJhY2sgaW4uIABSBwoFbG9naW4YAQ";
    let bytes = STANDARD_NO_PAD.decode(captured).unwrap();
    let details = ai::ErrorDetails::decode(bytes.as_slice()).unwrap();
    assert_eq!(details.error, 2, "ERROR_NOT_LOGGED_IN");
    assert_eq!(details.is_expected, Some(true));
    let custom = details.details.unwrap();
    assert_eq!(custom.title, "Authentication error");
    assert_eq!(custom.is_retryable, Some(false));
}

#[test]
fn captured_kv_ack_hex_decodes_as_agent_client_message() {
    let bytes = hex::decode("1a0408011a00").unwrap();
    let message = pb::AgentClientMessage::decode(bytes.as_slice()).unwrap();
    let Some(pb::agent_client_message::Message::KvClientMessage(kv)) = message.message else {
        panic!("expected KV client message")
    };
    assert_eq!(kv.id, 1);
    assert!(matches!(
        kv.message,
        Some(pb::kv_client_message::Message::SetBlobResult(_))
    ));
}

#[tokio::test]
async fn bidi_append_gzip_body_is_decompressed_before_protobuf_decode() {
    let (_directory, store) = fixtures::temp_store().await;
    let assets = PromptAssets::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("prompt/cursor")
            .as_path(),
    )
    .unwrap();
    let clients = NetworkClients::new(store.clone());
    let registry = TransportRegistry::new(
        store,
        Arc::new(fake_provider::FakeProvider::default()),
        PromptCompiler::new(assets),
    );
    let wire = ai::BidiAppendRequest {
        request_id: Some(ai::BidiRequestId {
            request_id: "gzip-request".into(),
        }),
        ..Default::default()
    }
    .encode_to_vec();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&wire).unwrap();
    let compressed = encoder.finish().unwrap();

    let response = cursor::router(registry, clients)
        .unwrap()
        .oneshot(
            Request::post("/aiserver.v1.BidiService/BidiAppend")
                .header(header::CONTENT_TYPE, "application/proto")
                .header(header::CONTENT_ENCODING, "gzip")
                .body(Body::from(compressed))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains("BidiAppend contains no AgentClientMessage"));
    assert!(!text.contains("protobuf decode error"));
}

/// Exercise both the run request and checkpoint acknowledgements through HTTP.
#[tokio::test]
async fn binary_bidi_append_runs_provider_and_returns_checkpoint() {
    let (_directory, store) = fixtures::temp_store().await;
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    let provider = fake_provider::FakeProvider::default();
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "binary-append-call".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("binary run completed".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let registry = TransportRegistry::new(
        store.clone(),
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    );
    let router = cursor::router(registry.clone(), NetworkClients::new(store.clone())).unwrap();
    let handle = registry.get_or_create("binary-append-run").await.unwrap();
    let mut output = handle.subscribe();
    let run = pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                conversation_id: Some("binary-append-conversation".into()),
                requested_model: Some(pb::RequestedModel {
                    model_id: "plugin:test/provider/model".into(),
                    ..Default::default()
                }),
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::UserMessageAction(
                        pb::UserMessageAction {
                            user_message: Some(pb::UserMessage {
                                text: "say hello".into(),
                                message_id: "binary-append-user".into(),
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
    };
    let append = |message: pb::AgentClientMessage, seqno| {
        let router = router.clone();
        async move {
            let wire = ai::BidiAppendRequest {
                request_id: Some(ai::BidiRequestId {
                    request_id: "binary-append-run".into(),
                }),
                append_seqno: seqno,
                data_binary: message.encode_to_vec(),
                ..Default::default()
            };
            // Also cover the production gzip decoder for the binary representation.
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&wire.encode_to_vec()).unwrap();
            let response = router
                .oneshot(
                    Request::post("/aiserver.v1.BidiService/BidiAppend")
                        .header(header::CONTENT_TYPE, "application/proto")
                        .header(header::CONTENT_ENCODING, "gzip")
                        .body(Body::from(encoder.finish().unwrap()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        }
    };
    append(run, 0).await;
    let mut checkpoint = false;
    let mut acknowledgements = 0;
    let mut text = String::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .expect("run output");
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            let terminal: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            assert!(terminal.get("error").is_none(), "{terminal}");
            break;
        }
        match pb::AgentServerMessage::decode(payload).unwrap().message {
            Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                if let Some(pb::interaction_update::Message::TextDelta(delta)) = update.message {
                    text.push_str(&delta.text);
                }
            }
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(_)) => {
                checkpoint = true
            }
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                assert!(matches!(
                    kv.message,
                    Some(pb::kv_server_message::Message::SetBlobArgs(_))
                ));
                acknowledgements += 1;
                append(
                    pb::AgentClientMessage {
                        message: Some(pb::agent_client_message::Message::KvClientMessage(
                            pb::KvClientMessage {
                                id: kv.id,
                                message: Some(pb::kv_client_message::Message::SetBlobResult(
                                    pb::SetBlobResult { error: None },
                                )),
                            },
                        )),
                    },
                    acknowledgements,
                )
                .await;
            }
            _ => {}
        }
    }
    assert!(checkpoint, "checkpoint must reach the client");
    assert_eq!(text, "binary run completed");
    assert!(acknowledgements > 0, "binary KV replies must be accepted");
    assert_eq!(provider.requests().len(), 1, "run must not be retried");
    registry.shutdown().await;
    let active: Option<String> = sqlx::query_scalar(
        "SELECT active_run_id FROM conversations WHERE conversation_id = 'binary-append-conversation'",
    ).fetch_one(store.pool()).await.unwrap();
    assert!(
        active.is_none(),
        "completed run must release the conversation"
    );
}
