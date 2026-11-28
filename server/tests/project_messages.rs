//! Project user messages are local presentation, not cross-agent execution.
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
    model::{ConversationId, MessageContent, ModelRequest},
    provider::{FinishReason, ModelEvent},
    store::{BlobId, Store},
};
use fake_provider::FakeProvider;
use prost::Message;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn queue(provider: &FakeProvider, id: &str, args: Option<Value>) {
    if let Some(args) = args {
        provider.push(vec![
            ModelEvent::Start {
                model_call_id: format!("model-{id}"),
            },
            ModelEvent::ToolCallStart {
                index: 0,
                call_id: id.into(),
                name: "SendMessage".into(),
            },
            ModelEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: args.to_string(),
            },
            ModelEvent::ToolCallEnd { index: 0 },
            ModelEvent::Done(FinishReason::ToolUse),
        ]);
    }
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: format!("done-{id}"),
        },
        ModelEvent::Done(FinishReason::Stop),
    ]);
}

fn request(project: bool, id: &str) -> pb::AgentRunRequest {
    pb::AgentRunRequest {
        conversation_id: Some("project-conversation".into()),
        requested_model: Some(pb::RequestedModel {
            model_id: "test-model".into(),
            ..Default::default()
        }),
        action: Some(pb::ConversationAction {
            action: Some(pb::conversation_action::Action::UserMessageAction(
                pb::UserMessageAction {
                    user_message: Some(pb::UserMessage {
                        text: "Give a project update".into(),
                        message_id: id.into(),
                        mode: pb::AgentMode::Agent as i32,
                        project_details: project.then(|| pb::ProjectDetails {
                            name: Some("test-project".into()),
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
    }
}

fn user(request: &mut pb::AgentRunRequest) -> &mut pb::UserMessage {
    let Some(pb::conversation_action::Action::UserMessageAction(action)) =
        request.action.as_mut().and_then(|a| a.action.as_mut())
    else {
        panic!("user request")
    };
    action.user_message.as_mut().unwrap()
}

async fn exchange(
    store: Store,
    provider: FakeProvider,
    request: pb::AgentRunRequest,
    id: &str,
) -> Vec<pb::AgentServerMessage> {
    let background = matches!(
        request.action.as_ref().and_then(|a| a.action.as_ref()),
        Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(_))
    );
    let registry = TransportRegistry::new(
        store.clone(),
        Arc::new(provider),
        PromptCompiler::new(PromptAssets::embedded().unwrap()),
    );
    let handle = registry.get_or_create(id).await.unwrap();
    let mut output = handle.subscribe();
    handle
        .command(TransportCommand::Append {
            seqno: 0,
            message: Box::new(pb::AgentClientMessage {
                message: Some(pb::agent_client_message::Message::RunRequest(request)),
            }),
        })
        .await
        .unwrap();
    let mut seqno = 1;
    let mut observed = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            assert!(
                !String::from_utf8_lossy(&payload).contains("error"),
                "{}",
                String::from_utf8_lossy(&payload)
            );
            break;
        }
        let server = pb::AgentServerMessage::decode(payload).unwrap();
        match &server.message {
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                let reply = match kv.message.as_ref().unwrap() {
                    pb::kv_server_message::Message::SetBlobArgs(_) => {
                        pb::kv_client_message::Message::SetBlobResult(pb::SetBlobResult {
                            error: None,
                        })
                    }
                    pb::kv_server_message::Message::GetBlobArgs(args) => {
                        pb::kv_client_message::Message::GetBlobResult(pb::GetBlobResult {
                            blob_data: store
                                .get_blob(&BlobId::from_bytes(&args.blob_id).unwrap())
                                .await
                                .unwrap(),
                            error: None,
                        })
                    }
                };
                handle
                    .command(TransportCommand::Append {
                        seqno,
                        message: Box::new(pb::AgentClientMessage {
                            message: Some(pb::agent_client_message::Message::KvClientMessage(
                                pb::KvClientMessage {
                                    id: kv.id,
                                    message: Some(reply),
                                },
                            )),
                        }),
                    })
                    .await
                    .unwrap();
                seqno += 1;
            }
            Some(pb::agent_server_message::Message::ExecServerMessage(exec))
                if background
                    && matches!(
                        exec.message,
                        Some(pb::exec_server_message::Message::RequestContextArgs(_))
                    ) =>
            {
                // A notification refreshes workspace context before the model
                // runs. SendMessage itself still emits no client execution.
                handle
                    .command(TransportCommand::Append {
                        seqno,
                        message: Box::new(pb::AgentClientMessage {
                            message: Some(pb::agent_client_message::Message::ExecClientMessage(
                                pb::ExecClientMessage {
                                    id: exec.id,
                                    message: Some(
                                        pb::exec_client_message::Message::RequestContextResult(
                                            pb::RequestContextResult {
                                                result: Some(
                                                    pb::request_context_result::Result::Success(
                                                        pb::RequestContextSuccess {
                                                            request_context: Some(
                                                                pb::RequestContext::default(),
                                                            ),
                                                            ..Default::default()
                                                        },
                                                    ),
                                                ),
                                            },
                                        ),
                                    ),
                                    ..Default::default()
                                },
                            )),
                        }),
                    })
                    .await
                    .unwrap();
                seqno += 1;
            }
            Some(pb::agent_server_message::Message::ExecServerMessage(_))
            | Some(pb::agent_server_message::Message::InteractionQuery(_)) => {
                panic!("unexpected client work in {id}: {server:?}")
            }
            _ => {}
        }
        observed.push(server);
    }
    observed
}

fn checkpoint(messages: &[pb::AgentServerMessage]) -> pb::ConversationStateStructure {
    messages
        .iter()
        .rev()
        .find_map(|m| match &m.message {
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) => {
                Some(state.clone())
            }
            _ => None,
        })
        .unwrap()
}

async fn run(project: bool, send: bool) -> (Vec<ModelRequest>, Vec<pb::AgentServerMessage>) {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = FakeProvider::default();
    queue(
        &provider,
        "send-user-once",
        send.then(|| json!({"message":"Visible project update"})),
    );
    let output = exchange(
        store,
        provider.clone(),
        request(project, "project-user"),
        "project-request",
    )
    .await;
    (provider.requests(), output)
}

#[tokio::test]
async fn project_root_exposes_send_message_and_persists_root_identity() {
    let (requests, messages) = run(true, false).await;
    assert!(
        requests[0]
            .prompt
            .tools
            .iter()
            .any(|t| t.name == "SendMessage"),
        "root project has no user-visible message tool"
    );
    let state = messages
        .iter()
        .rev()
        .find_map(|m| match &m.message {
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) => {
                Some(state)
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(state.is_root_project_conversation, Some(true));
}

#[tokio::test]
async fn ordinary_chat_does_not_offer_project_message_tool() {
    let (requests, _) = run(false, false).await;
    assert!(!requests[0]
        .prompt
        .tools
        .iter()
        .any(|t| t.name == "SendMessage"));
}

#[tokio::test]
async fn project_send_message_emits_one_success_card_without_client_execution() {
    let (_, messages) = run(true, true).await;
    let cards: Vec<_> = messages
        .iter()
        .filter_map(|m| match &m.message {
            Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                match &update.message {
                    Some(pb::interaction_update::Message::ToolCallCompleted(value)) => {
                        value.tool_call.as_ref()
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 1);
    let Some(pb::tool_call::Tool::SendMessageToolCall(card)) = &cards[0].tool else {
        panic!("SendMessage did not produce its native card")
    };
    let Some(pb::send_message_args::Message::Text(text)) =
        card.args.as_ref().and_then(|a| a.message.as_ref())
    else {
        panic!("message text missing")
    };
    assert_eq!(text.content, "Visible project update");
    assert!(matches!(
        card.result.as_ref().and_then(|r| r.result.as_ref()),
        Some(pb::send_message_result::Result::Success(_))
    ));
}

#[tokio::test]
async fn project_message_survives_restart_without_repeating_output_or_changing_history() {
    let (directory, store) = fixtures::temp_store().await;
    let provider = FakeProvider::default();
    queue(
        &provider,
        "first-send",
        Some(json!({"message":"Visible once"})),
    );
    let first = exchange(
        store.clone(),
        provider.clone(),
        request(true, "first-user"),
        "first-run",
    )
    .await;
    let first_state = checkpoint(&first);
    let turn_id = BlobId::from_bytes(first_state.turns.last().unwrap()).unwrap();
    let turn = pb::ConversationTurnStructure::decode(
        store.get_blob(&turn_id).await.unwrap().unwrap().as_slice(),
    )
    .unwrap();
    let Some(pb::conversation_turn_structure::Turn::AgentConversationTurn(turn)) = turn.turn else {
        panic!("agent turn")
    };
    assert_eq!(turn.send_message_step_indices, vec![0]);
    assert!(!first.iter().any(|message| match &message.message {
        Some(pb::agent_server_message::Message::InteractionUpdate(update)) => matches!(&update.message, Some(pb::interaction_update::Message::TextDelta(delta)) if delta.text.contains("Visible once")),
        _ => false,
    }));
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0].history).unwrap(),
        serde_json::to_value(&requests[1].history[..requests[0].history.len()]).unwrap()
    );
    assert_eq!(requests[0].prompt, requests[1].prompt);
    let history = store
        .load_current_messages(&ConversationId::new("project-conversation"))
        .await
        .unwrap();
    assert_eq!(history.iter().filter(|m| matches!(&m.content, MessageContent::ToolResult(result) if result.name == "SendMessage" && !result.is_error)).count(), 1);
    store.pool().close().await;
    let reopened = Store::connect(&format!(
        "sqlite://{}",
        directory.path().join("test.db").display()
    ))
    .await
    .unwrap();
    queue(&provider, "second-send", None);
    let second = exchange(
        reopened,
        provider.clone(),
        request(false, "second-user"),
        "second-run",
    )
    .await;
    let second_state = checkpoint(&second);
    assert_eq!(second_state.is_root_project_conversation, Some(true));
    assert!(
        second_state.turns.starts_with(&first_state.turns),
        "restart lost the prior SendMessage turn and indices"
    );
    let requests = provider.requests();
    assert_eq!(requests[1].prompt, requests[2].prompt);
    assert_eq!(
        serde_json::to_value(&requests[1].history).unwrap(),
        serde_json::to_value(&requests[2].history[..requests[1].history.len()]).unwrap()
    );
}

#[tokio::test]
async fn project_child_side_chat_and_best_of_n_do_not_offer_root_messages() {
    for kind in ["child", "side", "best", "subagent"] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = FakeProvider::default();
        queue(&provider, kind, None);
        let mut request = request(true, kind);
        match kind {
            "child" => {
                user(&mut request)
                    .project_details
                    .as_mut()
                    .unwrap()
                    .subagent = Some(pb::ProjectSubagentDetails::default())
            }
            "side" => {
                user(&mut request)
                    .project_details
                    .as_mut()
                    .unwrap()
                    .side_chat = Some(pb::ProjectSideChatDetails::default())
            }
            "best" => user(&mut request).best_of_n_group_id = Some("comparison".into()),
            _ => request.subagent_type_name = Some("generalPurpose".into()),
        }
        let output = exchange(store, provider.clone(), request, kind).await;
        assert!(
            !provider.requests()[0]
                .prompt
                .tools
                .iter()
                .any(|t| t.name == "SendMessage"),
            "{kind}"
        );
        assert_ne!(
            checkpoint(&output).is_root_project_conversation,
            Some(true),
            "{kind}"
        );
    }
}

fn action_request(action: pb::conversation_action::Action) -> pb::AgentRunRequest {
    let mut req = request(false, "unused");
    req.action.as_mut().unwrap().action = Some(action);
    req
}

async fn sent_step_ids(store: &Store, state: &pb::ConversationStateStructure) -> Vec<Vec<u8>> {
    let mut sent = Vec::new();
    for raw in &state.turns {
        let bytes = store
            .get_blob(&BlobId::from_bytes(raw).unwrap())
            .await
            .unwrap()
            .unwrap();
        let wrapper = pb::ConversationTurnStructure::decode(bytes.as_slice()).unwrap();
        if let Some(pb::conversation_turn_structure::Turn::AgentConversationTurn(turn)) =
            wrapper.turn
        {
            sent.extend(
                turn.send_message_step_indices
                    .into_iter()
                    .map(|index| turn.steps[index as usize].clone()),
            );
        }
    }
    sent
}

#[tokio::test]
async fn resume_and_background_completion_preserve_project_messages_with_or_without_client_state() {
    for background in [false, true] {
        for include_state in [false, true] {
            let (_directory, store) = fixtures::temp_store().await;
            let provider = FakeProvider::default();
            queue(
                &provider,
                "initial-send",
                Some(json!({"message":"Initial update"})),
            );
            let first = exchange(
                store.clone(),
                provider.clone(),
                request(true, "initial-user"),
                "initial-run",
            )
            .await;
            let state = checkpoint(&first);
            let sent = sent_step_ids(&store, &state).await;
            assert_eq!(sent.len(), 1);
            let action = if background {
                pb::conversation_action::Action::BackgroundTaskCompletionAction(
                    pb::BackgroundTaskCompletionAction {
                        completions: vec![pb::BackgroundTaskCompletion {
                            task_id: "12345".into(),
                            kind: pb::BackgroundTaskKind::Shell as i32,
                            status: pb::BackgroundTaskStatus::Success as i32,
                            title: "Short project check".into(),
                            detail: Some("exit code 0".into()),
                            reason: pb::BackgroundTaskCompletionReason::TaskFinished as i32,
                            ..Default::default()
                        }],
                    },
                )
            } else {
                pb::conversation_action::Action::ResumeAction(pb::ResumeAction::default())
            };
            let mut resumed = action_request(action);
            resumed.conversation_state = include_state.then_some(state);
            queue(
                &provider,
                "resumed-send",
                Some(json!({"message":"After resume"})),
            );
            let frames = exchange(store.clone(), provider.clone(), resumed, "resumed-run").await;
            let state = checkpoint(&frames);
            assert_eq!(state.is_root_project_conversation, Some(true));
            let after = sent_step_ids(&store, &state).await;
            assert_eq!(
                after.len(),
                2,
                "background={background}, state={include_state}"
            );
            assert!(after.starts_with(&sent));
            let requests = provider.requests();
            assert_eq!(requests[1].prompt, requests[2].prompt);
            if include_state {
                // Hydration assigns cursor-root IDs to model messages. Provider
                // content remains identical across that checkpoint boundary.
                let content = |r: &ModelRequest| {
                    r.history
                        .iter()
                        .map(|m| (m.role.clone(), m.content.clone()))
                        .collect::<Vec<_>>()
                };
                assert!(content(&requests[2]).starts_with(&content(&requests[1])));
            } else {
                assert!(requests[2].history.starts_with(&requests[1].history));
            }
            assert!(requests[3].history.starts_with(&requests[2].history));
        }
    }
}

#[tokio::test]
async fn compaction_keeps_project_identity_and_visible_message_indices() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = FakeProvider::default();
    queue(
        &provider,
        "before-summary",
        Some(json!({"message":"Remembered user update"})),
    );
    let first = exchange(
        store.clone(),
        provider.clone(),
        request(true, "first"),
        "first",
    )
    .await;
    let state = checkpoint(&first);
    let sent = sent_step_ids(&store, &state).await;
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "summarizer".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta(
            "Project user already received the update. Continue project work.".into(),
        ),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let mut summary = request(false, "summary");
    user(&mut summary).text = "/summarize".into();
    summary.conversation_state = Some(state);
    let frames = exchange(store.clone(), provider.clone(), summary, "summary").await;
    let compacted = checkpoint(&frames);
    assert_eq!(compacted.is_root_project_conversation, Some(true));
    assert_eq!(sent_step_ids(&store, &compacted).await, sent);
    assert!(provider.requests()[2].prompt.tools.is_empty());
    queue(
        &provider,
        "after-summary",
        Some(json!({"message":"Continued project work"})),
    );
    let frames = exchange(
        store.clone(),
        provider.clone(),
        request(false, "continue"),
        "continue",
    )
    .await;
    let after = checkpoint(&frames);
    assert_eq!(after.is_root_project_conversation, Some(true));
    let sent_after = sent_step_ids(&store, &after).await;
    assert!(sent_after.starts_with(&sent));
    assert_eq!(sent_after.len(), 2);
    assert_eq!(provider.requests()[0].prompt, provider.requests()[3].prompt);
}

#[tokio::test]
async fn imported_history_uses_the_latest_project_boundary_and_stops_at_ordinary_user() {
    for (kind, expected) in [
        ("simulated", true),
        ("ordinary", false),
        ("child", false),
        ("best", false),
    ] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = FakeProvider::default();
        let mut root = request(true, "root");
        let mut tail = request(false, "tail");
        match kind {
            "simulated" => user(&mut tail).is_simulated_msg = Some(true),
            "child" => {
                user(&mut tail).project_details = Some(pb::ProjectDetails {
                    subagent: Some(pb::ProjectSubagentDetails::default()),
                    ..Default::default()
                })
            }
            "best" => user(&mut tail).best_of_n_group_id = Some("group".into()),
            _ => {}
        }
        let mut turns = Vec::new();
        for message in [user(&mut root), user(&mut tail)] {
            let user_id = store.put_blob(&message.encode_to_vec(), &[]).await.unwrap();
            let turn = pb::ConversationTurnStructure {
                turn: Some(
                    pb::conversation_turn_structure::Turn::AgentConversationTurn(
                        pb::AgentConversationTurnStructure {
                            user_message: user_id.as_bytes().to_vec(),
                            ..Default::default()
                        },
                    ),
                ),
            };
            turns.push(
                store
                    .put_blob(&turn.encode_to_vec(), &[])
                    .await
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            );
        }
        let mut req = action_request(pb::conversation_action::Action::ResumeAction(
            pb::ResumeAction::default(),
        ));
        req.conversation_state = Some(pb::ConversationStateStructure {
            turns,
            mode: Some(pb::AgentMode::Agent as i32),
            ..Default::default()
        });
        queue(&provider, kind, None);
        let frames = exchange(store, provider.clone(), req, kind).await;
        assert_eq!(
            provider.requests()[0]
                .prompt
                .tools
                .iter()
                .any(|t| t.name == "SendMessage"),
            expected,
            "{kind}"
        );
        assert_eq!(
            checkpoint(&frames).is_root_project_conversation == Some(true),
            expected,
            "{kind}"
        );
    }
}
