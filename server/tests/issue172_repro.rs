//! Issue #172 reproduction probes. Assertions describe desired preservation behavior.
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
    provider::{FinishReason, ModelEvent},
    Error,
};
use prost::Message;
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy, Debug)]
enum Scenario {
    ManualPartial,
    ProviderFailure,
    ManualPendingTool,
    ManualMixedTools,
}

#[tokio::test]
async fn manual_cancel_then_followup_preserves_visible_text_and_completed_results() {
    reproduce(Scenario::ManualPartial).await;
}

#[tokio::test]
async fn provider_failure_then_resume_preserves_visible_text_and_completed_results() {
    reproduce(Scenario::ProviderFailure).await;
}

#[tokio::test]
async fn manual_cancel_during_tool_then_resume_preserves_completed_work() {
    reproduce(Scenario::ManualPendingTool).await;
}

#[tokio::test]
async fn cancelling_a_partial_tool_batch_preserves_success_and_cancels_only_pending_calls() {
    reproduce(Scenario::ManualMixedTools).await;
}

fn text_and_read() -> Vec<ModelEvent> {
    vec![
        ModelEvent::Start {
            model_call_id: "completed-cycle".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("COMPLETED_TEXT_172".into()),
        ModelEvent::TextEnd,
        ModelEvent::ToolCallStart {
            index: 0,
            call_id: "read-1".into(),
            name: "Read".into(),
        },
        ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: r#"{"path":"/tmp/a"}"#.into(),
        },
        ModelEvent::ToolCallEnd { index: 0 },
        ModelEvent::Done(FinishReason::ToolUse),
    ]
}

async fn send(
    handle: &cursor_server::cursor::TransportHandle,
    seq: &mut i64,
    message: pb::AgentClientMessage,
) {
    handle
        .command(TransportCommand::Append {
            seqno: *seq,
            message: Box::new(message),
        })
        .await
        .unwrap();
    *seq += 1;
}

async fn reproduce(scenario: Scenario) {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(text_and_read());
    let partial = vec![
        ModelEvent::Start {
            model_call_id: "interrupted-cycle".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("VISIBLE_BEFORE_STOP_172".into()),
    ];
    match scenario {
        Scenario::ManualPartial => provider.push_events_then_pending(partial),
        Scenario::ProviderFailure => {
            let mut events = partial.into_iter().map(Ok).collect::<Vec<_>>();
            events.push(Err(Error::Provider(
                "simulated connection reset during SSE".into(),
            )));
            provider.push_results(events);
            provider.push_results(vec![
                Ok(ModelEvent::Start {
                    model_call_id: "second-failed-attempt".into(),
                }),
                Ok(ModelEvent::TextStart),
                Ok(ModelEvent::TextDelta("SECOND_RETRY_TEXT_172".into())),
                Err(Error::Provider(
                    "simulated connection reset during SSE".into(),
                )),
            ]);
            // Exhaust the remaining retries, then exercise a separate Resume.
            for _ in 0..7 {
                provider.push_error(Error::Provider(
                    "simulated connection reset during SSE".into(),
                ));
            }
        }
        Scenario::ManualPendingTool | Scenario::ManualMixedTools => {
            let mut events = partial;
            if matches!(scenario, Scenario::ManualMixedTools) {
                events.extend([
                    ModelEvent::ToolCallStart {
                        index: 0,
                        call_id: "completed-in-batch".into(),
                        name: "Read".into(),
                    },
                    ModelEvent::ToolCallArgumentsDelta {
                        index: 0,
                        delta: r#"{"path":"/tmp/completed"}"#.into(),
                    },
                    ModelEvent::ToolCallEnd { index: 0 },
                ]);
            }
            events.extend([
                ModelEvent::TextEnd,
                ModelEvent::ToolCallStart {
                    index: 1,
                    call_id: "waiting-read".into(),
                    name: "Read".into(),
                },
                ModelEvent::ToolCallArgumentsDelta {
                    index: 1,
                    delta: r#"{"path":"/tmp/pending"}"#.into(),
                },
                ModelEvent::ToolCallEnd { index: 1 },
                ModelEvent::Done(FinishReason::ToolUse),
            ]);
            provider.push(events);
        }
    }
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "after-resume".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("finished".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let registry = TransportRegistry::new(
        store,
        Arc::new(provider.clone()),
        PromptCompiler::new(PromptAssets::embedded().unwrap()),
    );
    let first = registry.get_or_create("first-run").await.unwrap();
    let mut output = first.subscribe();
    let mut seq = 0;
    send(&first, &mut seq, start_request()).await;
    let mut checkpoint = None;
    let mut saw_visible_text = false;
    let mut cancellation_sent = false;
    let mut completed_reads = 0;
    let mut mixed_waiting = false;
    let mut mixed_completed = false;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(55), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            break;
        }
        let server = pb::AgentServerMessage::decode(payload).unwrap();
        match server.message {
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                acknowledge(&first, &mut seq, kv.id).await
            }
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) => {
                checkpoint = Some(state)
            }
            Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => {
                if completed_reads == 0
                    || (matches!(scenario, Scenario::ManualMixedTools) && completed_reads == 1)
                {
                    send(&first, &mut seq, read_result(exec.id)).await;
                    completed_reads += 1;
                } else if matches!(
                    scenario,
                    Scenario::ManualPendingTool | Scenario::ManualMixedTools
                ) {
                    if matches!(scenario, Scenario::ManualMixedTools) {
                        mixed_waiting = true;
                    } else {
                        send_cancel(&first, &mut seq).await;
                        cancellation_sent = true;
                    }
                } else {
                    panic!("unexpected extra tool before interruption");
                }
            }
            Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                if let Some(pb::interaction_update::Message::ToolCallCompleted(done)) =
                    &update.message
                {
                    mixed_completed |= done.call_id == "completed-in-batch";
                }
                if let Some(pb::interaction_update::Message::TextDelta(text)) = update.message {
                    if text.text.contains("VISIBLE_BEFORE_STOP_172") {
                        saw_visible_text = true;
                        if matches!(scenario, Scenario::ManualPartial) && !cancellation_sent {
                            send_cancel(&first, &mut seq).await;
                            cancellation_sent = true;
                        }
                    }
                }
            }
            _ => {}
        }
        // Define completion at the server's published acknowledgement, rather
        // than racing cancellation against an in-flight tool result.
        if mixed_waiting && mixed_completed && !cancellation_sent {
            send_cancel(&first, &mut seq).await;
            cancellation_sent = true;
        }
    }
    assert!(
        saw_visible_text,
        "marker must have reached the Cursor protocol output"
    );
    assert_eq!(
        completed_reads,
        if matches!(scenario, Scenario::ManualMixedTools) {
            2
        } else {
            1
        }
    );
    assert_eq!(
        cancellation_sent,
        !matches!(scenario, Scenario::ProviderFailure)
    );
    let before_resume_calls = provider.requests().len();
    let mut resume = resume_request(checkpoint.expect("client received checkpoint"));
    if matches!(scenario, Scenario::ManualPartial) {
        let Some(pb::agent_client_message::Message::RunRequest(request)) = resume.message.as_mut()
        else {
            unreachable!()
        };
        request.action = Some(pb::ConversationAction {
            action: Some(pb::conversation_action::Action::UserMessageAction(
                pb::UserMessageAction {
                    user_message: Some(pb::UserMessage {
                        text: "Repeat the previously produced text and read result.".into(),
                        message_id: "followup".into(),
                        mode: pb::AgentMode::Agent as i32,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
            ..Default::default()
        });
    }
    let resumed = registry.get_or_create("resumed-run").await.unwrap();
    let mut output = resumed.subscribe();
    let mut seq = 0;
    send(&resumed, &mut seq, resume).await;
    let mut replayed_tools = 0;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(12), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            break;
        }
        match pb::AgentServerMessage::decode(payload).unwrap().message {
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                acknowledge(&resumed, &mut seq, kv.id).await
            }
            Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => {
                replayed_tools += 1;
                send(&resumed, &mut seq, read_result(exec.id)).await;
            }
            _ => {}
        }
    }
    let requests = provider.requests();
    assert_eq!(requests.len(), before_resume_calls + 1);
    let recovered = &requests.last().unwrap().history;
    // Checkpoint hydration reconstructs internal message IDs; compare the
    // provider-visible roles/content across a new transport lifecycle.
    let visible = |history: &[cursor_server::model::ProjectedMessage]| {
        history
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect::<Vec<_>>()
    };
    assert!(
        visible(recovered).starts_with(&visible(&requests[1].history)),
        "completed provider content must remain an exact prefix"
    );
    let history = serde_json::to_string(recovered).unwrap();
    assert_eq!(
        history.matches("VISIBLE_BEFORE_STOP_172").count(),
        1,
        "visible text must be saved once"
    );
    if matches!(scenario, Scenario::ProviderFailure) {
        assert_eq!(history.matches("SECOND_RETRY_TEXT_172").count(), 1);
        assert!(
            serde_json::to_string(&requests[2].history)
                .unwrap()
                .contains("VISIBLE_BEFORE_STOP_172"),
            "automatic retry must also retain emitted text"
        );
        for pair in requests[1..before_resume_calls].windows(2) {
            assert!(
                pair[1].history.starts_with(&pair[0].history),
                "retry/resume must preserve the preceding request prefix"
            );
        }
    }
    if matches!(
        scenario,
        Scenario::ManualPendingTool | Scenario::ManualMixedTools
    ) {
        use cursor_server::model::ProjectedContent;
        let results = recovered
            .iter()
            .filter_map(|m| match &m.content {
                ProjectedContent::ToolResult(result) if result.call_id == "waiting-read" => {
                    Some(result)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error);
        assert!(results[0].content.contains("cancelled"));
        assert_eq!(
            replayed_tools, 0,
            "cancelled tools must not execute again on resume"
        );
    }
    if matches!(scenario, Scenario::ManualMixedTools) {
        use cursor_server::model::ProjectedContent;
        let result = recovered
            .iter()
            .find_map(|m| match &m.content {
                ProjectedContent::ToolResult(r) if r.call_id == "completed-in-batch" => Some(r),
                _ => None,
            })
            .expect("completed result in the same batch survives");
        assert!(!result.is_error);
        assert_eq!(result.content, "READ_RESULT_172");
    }
    let completed_text = history.contains("COMPLETED_TEXT_172");
    let completed_result = history.contains("READ_RESULT_172");
    let visible_text = history.contains("VISIBLE_BEFORE_STOP_172");
    eprintln!(
        "ISSUE172_EVIDENCE {}",
        serde_json::json!({
            "scenario": format!("{scenario:?}"), "visible_text_emitted":saw_visible_text,
            "completed_text_preserved":completed_text, "completed_result_preserved":completed_result,
            "interrupted_cycle_text_preserved":visible_text, "replayed_tools":replayed_tools,
            "calls_before_resume":before_resume_calls,
        })
    );
    registry.shutdown().await;
    assert!(completed_text, "lost an earlier completed model cycle");
    assert!(completed_result, "lost an earlier completed tool result");
    assert!(
        visible_text,
        "lost text already emitted to Cursor from the interrupted cycle"
    );
}

async fn send_cancel(handle: &cursor_server::cursor::TransportHandle, seq: &mut i64) {
    send(
        handle,
        seq,
        pb::AgentClientMessage {
            message: Some(pb::agent_client_message::Message::ConversationAction(
                pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::CancelAction(
                        pb::CancelAction::default(),
                    )),
                    ..Default::default()
                },
            )),
        },
    )
    .await;
}

fn start_request() -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::UserMessageAction(
                        pb::UserMessageAction {
                            user_message: Some(pb::UserMessage {
                                text: "read".into(),
                                message_id: "user-1".into(),
                                mode: pb::AgentMode::Agent as i32,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )),
                    ..Default::default()
                }),
                conversation_id: Some("conversation".into()),
                run_id: Some("first-run".into()),
                requested_model: Some(pb::RequestedModel {
                    model_id: "test-model".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
    }
}

fn resume_request(state: pb::ConversationStateStructure) -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::RunRequest(
            pb::AgentRunRequest {
                action: Some(pb::ConversationAction {
                    action: Some(pb::conversation_action::Action::ResumeAction(
                        pb::ResumeAction::default(),
                    )),
                    ..Default::default()
                }),
                conversation_state: Some(state),
                conversation_id: Some("conversation".into()),
                run_id: Some("resumed-run".into()),
                requested_model: Some(pb::RequestedModel {
                    model_id: "test-model".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
    }
}

fn read_result(id: u32) -> pb::AgentClientMessage {
    pb::AgentClientMessage {
        message: Some(pb::agent_client_message::Message::ExecClientMessage(
            pb::ExecClientMessage {
                id,
                message: Some(pb::exec_client_message::Message::ReadResult(
                    pb::ReadResult {
                        result: Some(pb::read_result::Result::Success(pb::ReadSuccess {
                            path: "/tmp/a".into(),
                            output: Some(pb::read_success::Output::Content(
                                "READ_RESULT_172".into(),
                            )),
                            ..Default::default()
                        })),
                    },
                )),
                ..Default::default()
            },
        )),
    }
}

async fn acknowledge(handle: &cursor_server::cursor::TransportHandle, seqno: &mut i64, id: u32) {
    handle
        .command(TransportCommand::Append {
            seqno: *seqno,
            message: Box::new(pb::AgentClientMessage {
                message: Some(pb::agent_client_message::Message::KvClientMessage(
                    pb::KvClientMessage {
                        id,
                        message: Some(pb::kv_client_message::Message::SetBlobResult(
                            pb::SetBlobResult { error: None },
                        )),
                    },
                )),
            }),
        })
        .await
        .unwrap();
    *seqno += 1;
}
