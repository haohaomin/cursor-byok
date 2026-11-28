//! Await must observe existing work without starting a replacement task.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;
use std::collections::{BTreeMap, HashSet};

use cursor_server::{
    cursor::{
        prompting::{Mode, PromptAssets},
        protocol::proto::agent::v1 as pb,
        tools::{
            runtime::{CursorToolRuntime, ExecContext},
            ToolBatchState, ToolDispatcher,
        },
    },
    model::ToolCall,
};
use serde_json::{json, Value};

async fn start(arguments: Value) -> cursor_server::cursor::tools::DispatchedTool {
    let call = ToolCall {
        index: 0,
        call_id: "await-existing-work".into(),
        model_call_id: "model-await".into(),
        name: "Await".into(),
        arguments_text: arguments.to_string(),
        arguments,
        argument_error: None,
    };
    let context = ExecContext {
        conversation_id: "parent".into(),
        root_conversation_id: "parent".into(),
        terminals_folder: "/workspace/terminals".into(),
        allow_subagents: true,
        ..Default::default()
    };
    ToolDispatcher::new(CursorToolRuntime::default())
        .start_batch(
            &[call],
            ToolBatchState {
                completed: &HashSet::new(),
                started: &HashSet::new(),
                response_text: "",
                response_thinking: "",
                goal_state: None,
            },
            &[],
            &BTreeMap::new(),
            &context,
        )
        .await
        .unwrap()
        .remove(0)
}

#[test]
fn await_is_available_to_the_agent() {
    let assets = PromptAssets::embedded().unwrap();
    assert!(assets
        .mode(Mode::Agent)
        .tools
        .iter()
        .any(|tool| tool.name == "Await"));
}

#[tokio::test]
async fn subagent_await_uses_bounded_observation_instead_of_task_resume() {
    let started = start(json!({"task_id": "child-existing", "block_until_ms": 0})).await;
    assert!(
        started.completion.is_none(),
        "Await failed before dispatch: {:?}",
        started.completion
    );
    let requests = started
        .messages
        .iter()
        .filter_map(|message| match message.message.as_ref() {
            Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => Some(exec),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    let Some(pb::exec_server_message::Message::SubagentAwaitArgs(args)) =
        requests[0].message.as_ref()
    else {
        panic!("must observe an existing child with SubagentAwaitArgs");
    };
    assert_eq!(args.agent_id, "child-existing");
    assert_eq!(args.timeout_ms, 0);
}

#[tokio::test]
async fn shell_await_starts_observing_terminal_output() {
    let started = start(json!({"task_id": "41", "block_until_ms": 0})).await;
    assert!(
        started.completion.is_none(),
        "Await failed before observing Shell output: {:?}",
        started.completion
    );
    assert!(started.messages.iter().any(|message| matches!(
        message.message.as_ref(),
        Some(pb::agent_server_message::Message::ExecServerMessage(_))
    )));
}

#[tokio::test]
async fn await_without_an_id_starts_a_timer_without_client_execution() {
    let started = start(json!({"block_until_ms": 100})).await;
    assert!(
        started.completion.is_none(),
        "Await failed instead of starting its timer: {:?}",
        started.completion
    );
    assert!(!started.messages.iter().any(|message| matches!(
        message.message.as_ref(),
        Some(pb::agent_server_message::Message::ExecServerMessage(_))
    )));
}

#[tokio::test]
async fn await_round_completes_through_transport_and_keeps_provider_history_prefix() {
    use cursor_server::{
        cursor::{
            prompting::PromptCompiler, protocol::connect, TransportCommand, TransportRegistry,
        },
        model::{ConversationId, MessageContent},
        provider::{FinishReason, ModelEvent},
    };
    use prost::Message;
    use std::{sync::Arc, time::Duration};
    for kind in ["shell", "subagent", "sleep", "interrupt"] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = fake_provider::FakeProvider::default();
        let args = match kind {
            "shell" => json!({"task_id":"23","block_until_ms":1000}),
            "subagent" => json!({"task_id":"child","block_until_ms":0}),
            "interrupt" => json!({"block_until_ms":10000}),
            _ => json!({"block_until_ms":5}),
        };
        provider.push(vec![
            ModelEvent::Start {
                model_call_id: "model-await".into(),
            },
            ModelEvent::ToolCallStart {
                index: 0,
                call_id: "wait-once".into(),
                name: "Await".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: args.to_string(),
            },
            ModelEvent::ToolCallEnd { index: 0 },
            ModelEvent::Done(FinishReason::ToolUse),
        ]);
        provider.push(vec![
            ModelEvent::Start {
                model_call_id: "model-after-await".into(),
            },
            ModelEvent::TextStart,
            ModelEvent::TextDelta("observed completion".into()),
            ModelEvent::TextEnd,
            ModelEvent::Done(FinishReason::Stop),
        ]);
        let registry = TransportRegistry::new(
            store.clone(),
            Arc::new(provider.clone()),
            PromptCompiler::new(PromptAssets::embedded().unwrap()),
        );
        let handle = registry.get_or_create("await-transport").await.unwrap();
        let mut output = handle.subscribe();
        let request = pb::AgentClientMessage {
            message: Some(pb::agent_client_message::Message::RunRequest(
                pb::AgentRunRequest {
                    conversation_id: Some("await-conversation".into()),
                    run_id: Some("await-transport".into()),
                    requested_model: Some(pb::RequestedModel {
                        model_id: "test-model".into(),
                        ..Default::default()
                    }),
                    action: Some(pb::ConversationAction {
                        action: Some(pb::conversation_action::Action::UserMessageAction(
                            pb::UserMessageAction {
                                user_message: Some(pb::UserMessage {
                                    text: "wait for existing work".into(),
                                    message_id: "user".into(),
                                    mode: pb::AgentMode::Agent as i32,
                                    ..Default::default()
                                }),
                                request_context: Some(pb::RequestContext {
                                    env: Some(pb::RequestContextEnv {
                                        terminals_folder: "/client/terminals".into(),
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
        };
        handle
            .command(TransportCommand::Append {
                seqno: 0,
                message: Box::new(request),
            })
            .await
            .unwrap();
        let mut seqno = 1;
        let mut reads = 0;
        let mut completed_cards = 0;
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
                .await
                .expect("Await transport must finish")
                .unwrap();
            let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
            if flags & connect::END_STREAM_FLAG != 0 {
                break;
            }
            let server = pb::AgentServerMessage::decode(payload).unwrap();
            let reply = match server.message {
                Some(pb::agent_server_message::Message::KvServerMessage(kv)) => Some(
                    pb::agent_client_message::Message::KvClientMessage(pb::KvClientMessage {
                        id: kv.id,
                        message: Some(pb::kv_client_message::Message::SetBlobResult(
                            pb::SetBlobResult { error: None },
                        )),
                    }),
                ),
                Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => {
                    let message = match exec.message {
                        Some(pb::exec_server_message::Message::ReadArgs(args)) => {
                            reads += 1;
                            let text = if kind == "shell" && reads == 1 {
                                "working"
                            } else {
                                "done\n---\nexit_code: 0\nelapsed_ms: 15\n---"
                            };
                            pb::exec_client_message::Message::ReadResult(pb::ReadResult {
                                result: Some(pb::read_result::Result::Success(pb::ReadSuccess {
                                    path: args.path,
                                    output: Some(pb::read_success::Output::Content(text.into())),
                                    file_size: text.len() as i64,
                                    ..Default::default()
                                })),
                            })
                        }
                        Some(pb::exec_server_message::Message::SubagentAwaitArgs(args)) => {
                            assert_eq!(kind, "subagent");
                            assert_eq!(args.agent_id, "child");
                            pb::exec_client_message::Message::SubagentAwaitResult(
                                pb::SubagentAwaitResult {
                                    result: Some(pb::subagent_await_result::Result::Complete(
                                        pb::SubagentAwaitComplete {
                                            agent_id: "child".into(),
                                            transcript_path: Some("/client/child.jsonl".into()),
                                            ..Default::default()
                                        },
                                    )),
                                },
                            )
                        }
                        _ => panic!("Await must never execute or resume a task"),
                    };
                    Some(pb::agent_client_message::Message::ExecClientMessage(
                        pb::ExecClientMessage {
                            id: exec.id,
                            exec_id: exec.exec_id,
                            message: Some(message),
                            ..Default::default()
                        },
                    ))
                }
                Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                    if kind == "interrupt"
                        && matches!(
                            update.message,
                            Some(pb::interaction_update::Message::ToolCallStarted(_))
                        )
                    {
                        Some(pb::agent_client_message::Message::ConversationAction(
                            pb::ConversationAction {
                                action: Some(pb::conversation_action::Action::UserMessageAction(
                                    pb::UserMessageAction {
                                        user_message: Some(pb::UserMessage {
                                            text: "new input releases waiting".into(),
                                            message_id: "new-user".into(),
                                            mode: pb::AgentMode::Agent as i32,
                                            ..Default::default()
                                        }),
                                        ..Default::default()
                                    },
                                )),
                                ..Default::default()
                            },
                        ))
                    } else {
                        if let Some(pb::interaction_update::Message::ToolCallCompleted(completed)) =
                            update.message
                        {
                            let tool = completed.tool_call.unwrap();
                            let Some(pb::tool_call::Tool::AwaitToolCall(await_call)) = tool.tool
                            else {
                                panic!("Await card missing")
                            };
                            if kind != "interrupt" {
                                assert!(matches!(
                                    await_call.result.unwrap().result,
                                    Some(pb::await_result::Result::Success(_))
                                ));
                            }
                            completed_cards += 1;
                        }
                        None
                    }
                }
                _ => None,
            };
            if let Some(message) = reply {
                handle
                    .command(TransportCommand::Append {
                        seqno,
                        message: Box::new(pb::AgentClientMessage {
                            message: Some(message),
                        }),
                    })
                    .await
                    .unwrap();
                seqno += 1;
            }
        }
        if kind != "interrupt" {
            assert_eq!(completed_cards, 1, "{kind}");
        }
        assert_eq!(
            reads,
            match kind {
                "shell" => 2,
                "subagent" => 1,
                _ => 0,
            }
        );
        let requests = provider.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            serde_json::to_value(&requests[0].history).unwrap(),
            serde_json::to_value(&requests[1].history[..requests[0].history.len()]).unwrap()
        );
        assert_eq!(
            requests[0].prompt.instructions,
            requests[1].prompt.instructions
        );
        let messages = store
            .load_current_messages(&ConversationId::new("await-conversation"))
            .await
            .unwrap();
        let results = messages
            .iter()
            .filter_map(|m| match &m.content {
                MessageContent::ToolResult(result) => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        if kind == "interrupt" {
            assert!(results[0].is_error);
            assert!(serde_json::to_string(&requests[1].history)
                .unwrap()
                .contains("new input releases waiting"));
        } else {
            assert!(!results[0].is_error);
            assert!(results[0].content.contains("completed"));
        }
    }
}
