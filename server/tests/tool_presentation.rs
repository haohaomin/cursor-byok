//! Exercises live Cursor ordering and checkpoint replay without a desktop client.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use cursor_server::{
    cursor::{
        prompting::{PromptAssets, PromptCompiler},
        protocol::{connect, proto::agent::v1 as pb},
        TransportCommand, TransportRegistry,
    },
    model::{ConversationId, MessageContent},
    provider::{FinishReason, ModelEvent},
    store::BlobId,
};
use prost::Message;
use std::{collections::HashSet, sync::Arc, time::Duration};

#[tokio::test]
async fn shell_success_precedes_trailing_text_and_thinking() {
    exercise("success", 1).await;
}
#[tokio::test]
async fn shell_failure_precedes_trailing_text_and_thinking() {
    exercise("failure", 1).await;
}
#[tokio::test]
async fn shell_rejection_precedes_trailing_text_and_thinking() {
    exercise("rejected", 1).await;
}
#[tokio::test]
async fn background_ack_releases_text_without_waiting_for_process_exit() {
    exercise("background", 1).await;
}
#[tokio::test]
async fn all_tools_complete_before_interleaved_text_is_released() {
    exercise("success", 2).await;
}

#[tokio::test]
async fn cancel_drops_deferred_narration_and_does_not_start_another_model_call() {
    exercise("cancel", 1).await;
}

#[tokio::test]
async fn provider_retry_closes_partial_tools_before_releasing_narration() {
    exercise("retry", 1).await;
}

#[tokio::test]
async fn reversed_parallel_results_settle_every_card_before_text() {
    exercise("out_of_order", 2).await;
}

async fn exercise(outcome: &str, count: usize) {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let mut events = vec![
        ModelEvent::Start {
            model_call_id: "model-0".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("before".into()),
        ModelEvent::TextEnd,
    ];
    for index in 0..count {
        events.extend([
            ModelEvent::ToolCallStart {
                index,
                call_id: format!("shell-{index}"),
                name: "Shell".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index,
                delta: r#"{"command":"printf test","block_until_ms":10000}"#.into(),
            },
            ModelEvent::ToolCallEnd { index },
            ModelEvent::TextStart,
            ModelEvent::TextDelta(format!("after-{index}")),
            ModelEvent::TextEnd,
            ModelEvent::ThinkingStart,
            ModelEvent::ThinkingDelta(format!("thinking-{index}")),
            ModelEvent::ThinkingEnd,
        ]);
    }
    events.push(ModelEvent::Done(FinishReason::ToolUse));
    if outcome == "retry" {
        events.pop();
        provider.push_results(
            events
                .into_iter()
                .map(Ok)
                .chain([Err(cursor_server::Error::Provider(
                    "stream disconnected".into(),
                ))])
                .collect(),
        );
    } else {
        provider.push(events);
    }
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "model-1".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("final".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    let registry = TransportRegistry::new(
        store.clone(),
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    );
    let handle = registry
        .get_or_create("presentation-request")
        .await
        .unwrap();
    let mut output = handle.subscribe();
    let mut seqno = 0;
    handle
        .command(TransportCommand::Append {
            seqno,
            message: Box::new(client_run()),
        })
        .await
        .unwrap();
    seqno += 1;
    let mut unfinished = HashSet::new();
    let mut completed = 0;
    let mut transcript = Vec::new();
    let mut checkpoint = None;
    let mut exec_ids = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            if outcome == "cancel" {
                let error: serde_json::Value = serde_json::from_slice(&payload).unwrap();
                assert_eq!(error["error"]["code"], "canceled");
                assert_eq!(transcript, ["text:before"]);
                assert_eq!(provider.requests().len(), 1);
                return;
            }
            assert_eq!(payload.as_ref(), b"{}");
            break;
        }
        let server = pb::AgentServerMessage::decode(payload).unwrap();
        let reply = match server.message {
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                Some(pb::AgentClientMessage {
                    message: Some(pb::agent_client_message::Message::KvClientMessage(
                        pb::KvClientMessage {
                            id: kv.id,
                            message: Some(pb::kv_client_message::Message::SetBlobResult(
                                pb::SetBlobResult { error: None },
                            )),
                        },
                    )),
                })
            }
            Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => {
                assert!(matches!(
                    exec.message,
                    Some(pb::exec_server_message::Message::ShellStreamArgs(_))
                ));
                if outcome == "out_of_order" {
                    exec_ids.push(exec.id);
                    if exec_ids.len() < count {
                        continue;
                    }
                    // Return both results without waiting for their completion events.
                    // The store can settle the round ahead of the output consumer.
                    handle
                        .command(TransportCommand::Append {
                            seqno,
                            message: Box::new(shell_result(exec_ids[1], "success")),
                        })
                        .await
                        .unwrap();
                    seqno += 1;
                    Some(shell_result(exec_ids[0], "success"))
                } else if outcome == "cancel" {
                    Some(pb::AgentClientMessage {
                        message: Some(pb::agent_client_message::Message::ConversationAction(
                            pb::ConversationAction {
                                action: Some(pb::conversation_action::Action::CancelAction(
                                    pb::CancelAction::default(),
                                )),
                                ..Default::default()
                            },
                        )),
                    })
                } else {
                    Some(shell_result(exec.id, outcome))
                }
            }
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) => {
                checkpoint = Some(state);
                None
            }
            Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                match update.message {
                    Some(pb::interaction_update::Message::PartialToolCall(call)) => {
                        unfinished.insert(call.call_id);
                    }
                    Some(pb::interaction_update::Message::ToolCallCompleted(call)) => {
                        assert!(unfinished.remove(&call.call_id));
                        completed += 1;
                        let tool = call.tool_call.unwrap().tool.unwrap();
                        if outcome == "retry" {
                            let pb::tool_call::Tool::McpToolCall(mcp) = tool else {
                                panic!("expected failed partial call");
                            };
                            assert!(matches!(
                                mcp.result.unwrap().result,
                                Some(pb::mcp_tool_result::Result::Error(_))
                            ));
                        } else {
                            let pb::tool_call::Tool::ShellToolCall(shell) = tool else {
                                panic!("expected Shell");
                            };
                            let result = shell.result.unwrap();
                            match outcome {
                                "failure" => assert!(matches!(
                                    result.result,
                                    Some(pb::shell_result::Result::Failure(_))
                                )),
                                "rejected" => assert!(matches!(
                                    result.result,
                                    Some(pb::shell_result::Result::Rejected(_))
                                )),
                                "background" => assert_eq!(result.is_background, Some(true)),
                                _ => assert!(matches!(
                                    result.result,
                                    Some(pb::shell_result::Result::Success(_))
                                )),
                            }
                        }
                        transcript.push(format!("tool:{}", call.call_id));
                    }
                    Some(pb::interaction_update::Message::TextDelta(delta)) => {
                        assert!(
                            unfinished.is_empty(),
                            "Cursor cancels unfinished tool cards on text: {unfinished:?}"
                        );
                        if delta.text == "before" {
                            assert_eq!(completed, 0);
                        } else {
                            assert_eq!(completed, count);
                        }
                        transcript.push(format!("text:{}", delta.text));
                    }
                    Some(pb::interaction_update::Message::ThinkingDelta(delta)) => {
                        assert!(
                            unfinished.is_empty(),
                            "Cursor cancels unfinished tool cards on thinking"
                        );
                        assert_eq!(completed, count);
                        transcript.push(format!("thinking:{}", delta.text));
                    }
                    Some(pb::interaction_update::Message::ThinkingCompleted(_)) => {
                        assert!(unfinished.is_empty());
                    }
                    _ => {}
                }
                None
            }
            _ => None,
        };
        if let Some(message) = reply {
            handle
                .command(TransportCommand::Append {
                    seqno,
                    message: Box::new(message),
                })
                .await
                .unwrap();
            seqno += 1;
        }
    }
    assert_eq!(completed, count);
    let mut expected = vec!["text:before".to_string()];
    if outcome == "out_of_order" {
        expected.extend((0..count).rev().map(|i| format!("tool:shell-{i}")));
    } else {
        expected.extend((0..count).map(|i| format!("tool:shell-{i}")));
    }
    for i in 0..count {
        expected.extend([format!("text:after-{i}"), format!("thinking:thinking-{i}")]);
    }
    expected.push("text:final".into());
    assert_eq!(transcript, expected);
    // Replay uses the exact same presentation order as the live stream.
    let mut replay = Vec::new();
    for id in checkpoint.unwrap().turns {
        let bytes = store
            .get_blob(&BlobId::from_bytes(&id).unwrap())
            .await
            .unwrap()
            .unwrap();
        let wrapper = pb::ConversationTurnStructure::decode(bytes.as_slice()).unwrap();
        if let Some(pb::conversation_turn_structure::Turn::AgentConversationTurn(turn)) =
            wrapper.turn
        {
            for id in turn.steps {
                let bytes = store
                    .get_blob(&BlobId::from_bytes(&id).unwrap())
                    .await
                    .unwrap()
                    .unwrap();
                let step = pb::ConversationStep::decode(bytes.as_slice()).unwrap();
                match step.message.unwrap() {
                    pb::conversation_step::Message::AssistantMessage(m) => {
                        replay.push(format!("text:{}", m.text))
                    }
                    pb::conversation_step::Message::ThinkingMessage(m) => {
                        replay.push(format!("thinking:{}", m.text))
                    }
                    pb::conversation_step::Message::ToolCall(t) => {
                        replay.push(format!("tool:{}", t.tool_call_id.unwrap()))
                    }
                }
            }
        }
    }
    assert_eq!(replay, expected);
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].history.starts_with(&requests[0].history));
    let messages = store
        .load_current_messages(&ConversationId::new("presentation-conversation"))
        .await
        .unwrap();
    let expected_text = format!(
        "before{}",
        (0..count).map(|i| format!("after-{i}")).collect::<String>()
    );
    let has_attempt_text = messages.iter().any(
        |m| matches!(&m.content, MessageContent::Assistant { text, .. } if text == &expected_text),
    );
    if outcome == "retry" {
        assert_eq!(
            requests[0], requests[1],
            "retry must use the same canonical history"
        );
        assert!(
            !has_attempt_text,
            "failed attempt text is presentation-only"
        );
    } else {
        assert!(has_attempt_text);
    }
}

fn shell_result(id: u32, outcome: &str) -> pb::AgentClientMessage {
    let event = match outcome {
        "background" => pb::shell_stream::Event::Backgrounded(pb::ShellStreamBackgrounded {
            shell_id: 42,
            command: "printf test".into(),
            working_directory: "/tmp".into(),
            pid: Some(1234),
            ms_to_wait: Some(10000),
            reason: Some(pb::ShellBackgroundReason::Timeout as i32),
        }),
        "rejected" => pb::shell_stream::Event::Rejected(pb::ShellRejected {
            reason: "User rejected".into(),
            ..Default::default()
        }),
        _ => pb::shell_stream::Event::Exit(pb::ShellStreamExit {
            code: if outcome == "failure" { 1 } else { 0 },
            ..Default::default()
        }),
    };
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::ExecClientMessage(
            pb::ExecClientMessage {
                id,
                message: Some(pb::exec_client_message::Message::ShellStream(
                    pb::ShellStream { event: Some(event) },
                )),
                ..Default::default()
            },
        )),
    }
}

fn client_run() -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                conversation_id: Some("presentation-conversation".into()),
                run_id: Some("presentation-request".into()),
                requested_model: Some(pb::RequestedModel {
                    model_id: "test-model".into(),
                    ..Default::default()
                }),
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::UserMessageAction(
                        pb::UserMessageAction {
                            user_message: Some(pb::UserMessage {
                                text: "test tool presentation".into(),
                                message_id: "presentation-user".into(),
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
