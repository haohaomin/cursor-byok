//! Large valid history and PNG attachments survive every supported append shape.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::{io::Write, sync::Arc, time::Duration};

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use cursor_server::{
    api::cursor,
    cursor::{
        conversation::TransportCommand,
        prompting::{PromptAssets, PromptCompiler},
        protocol::{
            connect,
            proto::{agent::v1 as pb, aiserver::v1 as ai},
        },
        transport::TransportRegistry,
    },
    model::{ContentPart, ProjectedContent},
    network::NetworkClients,
    store::BlobId,
};
use flate2::{write::GzEncoder, Compression};
use image::ImageEncoder;
use prost::Message;
use tower::ServiceExt;

fn large_png() -> Vec<u8> {
    let mut seed = 0x1234_5678_u32;
    let pixels: Vec<u8> = (0..1024 * 1024 * 3)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, 1024, 1024, image::ExtendedColorType::Rgb8)
        .unwrap();
    assert!(png.len() > 3 * 1024 * 1024);
    png
}

#[tokio::test]
async fn large_history_and_png_reach_provider_unchanged() {
    let history = "previous conversation\n".repeat(200_000);
    assert!(history.len() > 4 * 1024 * 1024);
    let history_blob = serde_json::to_vec(&serde_json::json!({
        "role": "user", "content": history
    }))
    .unwrap();
    let history_id = BlobId::digest(&history_blob).as_bytes().to_vec();
    let system_blob = br#"{"role":"system","content":"You are a test assistant."}"#.to_vec();
    let system_id = BlobId::digest(&system_blob).as_bytes().to_vec();
    let png = large_png();

    for (binary, framed, gzip) in [
        (true, false, false),
        (true, true, true),
        (false, false, true),
        (false, true, false),
    ] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = fake_provider::FakeProvider::default();
        provider.push_pending();
        let assets = PromptAssets::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"),
        )
        .unwrap();
        let registry = TransportRegistry::new(
            store.clone(),
            Arc::new(provider.clone()),
            PromptCompiler::new(assets),
        );
        let router = cursor::router(registry.clone(), NetworkClients::new(store)).unwrap();
        let handle = registry.get_or_create("large-append").await.unwrap();
        let mut output = handle.subscribe();
        let run = pb::AgentClientMessage {
            message: Some(pb::agent_client_message::Message::RunRequest(
                pb::AgentRunRequest {
                    conversation_id: Some("large-append-conversation".into()),
                    requested_model: Some(pb::RequestedModel {
                        model_id: "plugin:test/provider/model".into(),
                        parameters: vec![pb::requested_model::ModelParameterValue {
                            id: "context".into(),
                            value: "20000000".into(),
                        }],
                        ..Default::default()
                    }),
                    conversation_state: Some(pb::ConversationStateStructure {
                        root_prompt_messages_json: vec![system_id.clone(), history_id.clone()],
                        ..Default::default()
                    }),
                    pre_fetched_blobs: vec![
                        pb::PreFetchedBlob {
                            id: system_id.clone(),
                            value: system_blob.clone(),
                        },
                        pb::PreFetchedBlob {
                            id: history_id.clone(),
                            value: history_blob.clone(),
                        },
                    ],
                    action: Some(pb::ConversationAction {
                        action: Some(pb::conversation_action::Action::UserMessageAction(
                            pb::UserMessageAction {
                                user_message: Some(pb::UserMessage {
                                    text: "inspect this image after the previous conversation"
                                        .into(),
                                    message_id: "large-append-user".into(),
                                    mode: pb::AgentMode::Agent as i32,
                                    selected_context: Some(pb::SelectedContext {
                                        selected_images: vec![pb::SelectedImage {
                                            mime_type: "image/png".into(),
                                            data_or_blob_id: Some(
                                                pb::selected_image::DataOrBlobId::Data(png.clone()),
                                            ),
                                            ..Default::default()
                                        }],
                                        ..Default::default()
                                    }),
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
        .encode_to_vec();
        let append = ai::BidiAppendRequest {
            request_id: Some(ai::BidiRequestId {
                request_id: "large-append".into(),
            }),
            data: if binary {
                String::new()
            } else {
                hex::encode(&run)
            },
            data_binary: if binary { run } else { Vec::new() },
            ..Default::default()
        };
        let mut wire = if framed {
            connect::encode_message(&append).unwrap().to_vec()
        } else {
            append.encode_to_vec()
        };
        assert!(wire.len() > 7 * 1024 * 1024 && wire.len() < 64 * 1024 * 1024);
        if gzip {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            encoder.write_all(&wire).unwrap();
            wire = encoder.finish().unwrap();
        }
        let response = router
            .oneshot(
                Request::post("/aiserver.v1.BidiService/BidiAppend")
                    .header(header::CONTENT_TYPE, "application/proto")
                    .header(
                        header::CONTENT_ENCODING,
                        if gzip { "gzip" } else { "identity" },
                    )
                    .header(header::CONTENT_LENGTH, wire.len())
                    .body(Body::from(wire))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let requests = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let requests = provider.requests();
                if !requests.is_empty() {
                    break requests;
                }
                tokio::select! {
                    frame = output.recv() => {
                        let frame = frame.expect("run output");
                        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
                        assert_eq!(flags & connect::END_STREAM_FLAG, 0, "run ended before provider: {}", String::from_utf8_lossy(&payload));
                        if let Some(pb::agent_server_message::Message::KvServerMessage(kv)) = pb::AgentServerMessage::decode(payload).unwrap().message {
                            assert!(matches!(kv.message, Some(pb::kv_server_message::Message::SetBlobArgs(_))), "unexpected KV request");
                            handle.command(TransportCommand::Append {
                                seqno: i64::from(kv.id),
                                message: Box::new(pb::AgentClientMessage {
                                    message: Some(pb::agent_client_message::Message::KvClientMessage(pb::KvClientMessage {
                                        id: kv.id,
                                        message: Some(pb::kv_client_message::Message::SetBlobResult(pb::SetBlobResult { error: None })),
                                    })),
                                }),
                            }).await.unwrap();
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        })
        .await
        .expect("large valid append must reach the provider");
        assert_eq!(requests.len(), 1);
        let parts: Vec<_> = requests[0]
            .history
            .iter()
            .flat_map(|message| match &message.content {
                ProjectedContent::Parts(parts) => parts.iter().collect(),
                _ => Vec::new(),
            })
            .collect();
        assert!(
            parts
                .iter()
                .any(|part| matches!(part, ContentPart::Text { text } if text == &history)),
            "history must not be truncated"
        );
        assert!(parts.iter().any(|part| matches!(part, ContentPart::Image { mime_type, data } if mime_type == "image/png" && data == &png)), "PNG bytes must be preserved");
        registry.shutdown().await;
    }
}
