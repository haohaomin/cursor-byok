//! Verifies Cursor-selected subagent models reach the provider without a fixed model whitelist.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::sync::Arc;

use cursor_server::{
    cursor::{
        prompting::{PromptAssets, PromptCompiler},
        protocol::connect,
        protocol::proto::agent::v1 as pb,
        TransportCommand, TransportRegistry,
    },
    model::{ContentPart, ModelConfigInput, ProjectedContent},
    provider::{FinishReason, ModelEvent},
};
use prost::Message;

#[tokio::test]
async fn client_subagent_models_reach_provider_context() {
    check_client_model_context(false).await;
}

#[tokio::test]
async fn hash_only_client_models_get_local_names_without_exposing_other_config() {
    check_client_model_context(true).await;
}

async fn check_client_model_context(hash_only: bool) {
    let (_store_dir, store) = fixtures::temp_store().await;
    let mut run = user_run();
    let mut expected_id = "my-custom-model".to_string();
    if hash_only {
        let input: ModelConfigInput = serde_json::from_value(serde_json::json!({
            "display_name": "My Custom Model",
            "type": "openai", "base_url": "https://private-provider.invalid/v1",
            "api_key": "secret-not-for-context", "tooltip_data": "private-tooltip",
            "model_id": "private-provider-slug", "openai_endpoint": cursor_server::model::OPENAI_CHAT_ENDPOINT
        })).unwrap();
        let model = store.create_model(&input).await.unwrap();
        let mut unselected = input;
        unselected.display_name = "Unselected Local Model".into();
        unselected.model_id = "unselected-slug".into();
        store.create_model(&unselected).await.unwrap();
        expected_id = model.model_hash;
        let Some(pb::agent_client_message::Message::RunRequest(request)) = run.message.as_mut()
        else {
            unreachable!()
        };
        request.selected_subagent_models[0].model_id = expected_id.clone();
        request.selected_subagent_model_details.clear();
        request
            .subagent_model_overrides
            .push(pb::SubagentModelOverride {
                subagent_type: "generalPurpose".into(),
                selection: Some(pb::subagent_model_override::Selection::Model(
                    pb::RequestedModel {
                        model_id: expected_id.clone(),
                        ..Default::default()
                    },
                )),
            });
    }
    let provider = fake_provider::FakeProvider::default();
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "call-1".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("ok".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let assets = PromptAssets::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("prompt/cursor")
            .as_path(),
    )
    .unwrap();

    let registry = TransportRegistry::new(
        store,
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    );
    let handle = registry.get_or_create("rules-request").await.unwrap();
    let mut output = handle.subscribe();
    handle
        .command(TransportCommand::Append {
            seqno: 0,
            message: Box::new(run),
        })
        .await
        .unwrap();

    let mut append_seqno = 1;
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), output.recv())
            .await
            .expect("run finishes within timeout")
            .expect("output stays open until EndStream");
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            break;
        }
        // The Run waits for the client to confirm every conversation Blob write,
        // so the stream only advances once each KvServerMessage is acknowledged.
        if let Some(pb::agent_server_message::Message::KvServerMessage(kv)) =
            pb::AgentServerMessage::decode(payload).unwrap().message
        {
            handle
                .command(TransportCommand::Append {
                    seqno: append_seqno,
                    message: Box::new(set_blob_result(kv.id)),
                })
                .await
                .unwrap();
            append_seqno += 1;
        }
    }

    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    let context_texts = requests[0]
        .history
        .iter()
        .filter(|message| message.message_id.starts_with("request-context:"))
        .map(|message| {
            let ProjectedContent::Parts(parts) = &message.content else {
                panic!("request context message must be parts")
            };
            let [ContentPart::Text { text }] = parts.as_slice() else {
                panic!("request context message must be one text part")
            };
            text.clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        context_texts.len(),
        1,
        "exactly one request-context message is projected"
    );
    assert!(
        context_texts[0].contains(&expected_id),
        "client model is missing: {}",
        context_texts[0]
    );
    assert!(
        context_texts[0].contains("My Custom Model"),
        "missing name: {}",
        context_texts[0]
    );
    for private in [
        "secret-not-for-context",
        "private-provider.invalid",
        "private-tooltip",
        "private-provider-slug",
        "Unselected Local Model",
        "unselected-slug",
    ] {
        assert!(!context_texts[0].contains(private), "leaked {private}");
    }
    let task = requests[0]
        .prompt
        .tools
        .iter()
        .find(|t| t.name == "Task")
        .unwrap();
    assert!(!task.description.contains("claude-opus-5-thinking-high"));
    assert!(
        !task.description.contains("my-custom-model"),
        "dynamic list belongs in context, not stable tool definitions"
    );

    registry.shutdown().await;
}

fn set_blob_result(id: u32) -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::KvClientMessage(
            pb::KvClientMessage {
                id,
                message: Some(pb::kv_client_message::Message::SetBlobResult(
                    pb::SetBlobResult { error: None },
                )),
            },
        )),
    }
}

fn user_run() -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::UserMessageAction(
                        pb::UserMessageAction {
                            user_message: Some(pb::UserMessage {
                                text: "hello".into(),
                                message_id: "rules-user".into(),
                                mode: pb::AgentMode::Agent as i32,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
                conversation_id: Some("rules-conversation".into()),
                run_id: Some("rules-request".into()),
                selected_subagent_models: vec![pb::RequestedModel {
                    model_id: "my-custom-model".into(),
                    ..Default::default()
                }],
                selected_subagent_model_details: vec![pb::ModelDetails {
                    model_id: "my-custom-model".into(),
                    display_name: "My Custom Model".into(),
                    ..Default::default()
                }],
                requested_model: Some(pb::RequestedModel {
                    model_id: "test-model".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
    }
}
