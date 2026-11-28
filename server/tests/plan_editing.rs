//! #139: Plan mode edits the known document instead of creating a second plan.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use cursor_server::{
    cursor::{
        prompting::{Mode, PromptAssets, PromptCompiler},
        protocol::{connect, proto::agent::v1 as pb},
        TransportCommand, TransportRegistry,
    },
    model::{ModelSpec, ProjectedContent},
    provider::{FinishReason, ModelEvent},
    store::{BlobId, Store},
};
use fake_provider::FakeProvider;
use prost::Message;
use serde_json::json;
use std::{sync::Arc, time::Duration};

const PLAN_URI: &str = "file:///workspace/.cursor/plans/existing%20plan.md";

// Independent wire contract, so the missing field is testable on the baseline.
#[derive(Clone, PartialEq, Message)]
struct WriteContract {
    #[prost(bool, tag = "7")]
    plan_mode: bool,
}

#[test]
fn plan_offers_existing_document_edits_and_create_plan_only_creates() {
    let compiler = PromptCompiler::new(PromptAssets::embedded().unwrap());
    let prompt = compiler
        .prompt_spec(Mode::Plan, &ModelSpec::new("test-model"), &[], false)
        .unwrap();
    for name in ["Write", "StrReplace"] {
        assert!(
            prompt.tools.iter().any(|t| t.name == name),
            "Plan does not offer {name}"
        );
    }
    let create = prompt
        .tools
        .iter()
        .find(|t| t.name == "CreatePlan")
        .unwrap();
    assert!(!create
        .description
        .contains("call this tool with the complete revised plan"));
}

type Exchange = (
    Vec<pb::ExecServerMessage>,
    Vec<cursor_server::model::ModelRequest>,
    Vec<pb::ToolCall>,
    pb::ConversationStateStructure,
);

fn request(mode: pb::AgentMode, id: &str) -> pb::AgentRunRequest {
    pb::AgentRunRequest {
        conversation_id: Some("plan-edit-conversation".into()),
        requested_model: Some(pb::RequestedModel {
            model_id: "test-model".into(),
            ..Default::default()
        }),
        action: Some(pb::ConversationAction {
            action: Some(pb::conversation_action::Action::UserMessageAction(
                pb::UserMessageAction {
                    user_message: Some(pb::UserMessage {
                        text: "Revise the existing plan".into(),
                        message_id: id.into(),
                        mode: mode as i32,
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

async fn edit(
    mode: pb::AgentMode,
    path: &str,
) -> (
    Vec<pb::ExecServerMessage>,
    Vec<cursor_server::model::ModelRequest>,
) {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = FakeProvider::default();
    let (execs, requests, _, _) = exchange(
        store,
        provider,
        request(mode, "user-edit"),
        "plan-edit-run",
        "StrReplace",
        json!({"path":path,"old_string":"before","new_string":"after"}),
    )
    .await;
    (execs, requests)
}

async fn exchange(
    store: Store,
    provider: FakeProvider,
    request: pb::AgentRunRequest,
    id: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> Exchange {
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: format!("edit-model-{id}"),
        },
        ModelEvent::ToolCallStart {
            index: 0,
            call_id: format!("edit-existing-plan-{id}"),
            name: tool.into(),
        },
        ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: arguments.to_string(),
        },
        ModelEvent::ToolCallEnd { index: 0 },
        ModelEvent::Done(FinishReason::ToolUse),
    ]);
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: format!("done-{id}"),
        },
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let registry = TransportRegistry::new(
        store.clone(),
        Arc::new(provider.clone()),
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
    let mut execs = Vec::new();
    let mut cards = Vec::new();
    let mut state = None;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, data) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            let end: serde_json::Value = serde_json::from_slice(&data).unwrap();
            assert!(end.get("error").is_none(), "{end}");
            break;
        }
        let server = pb::AgentServerMessage::decode(data).unwrap();
        let reply = match server.message {
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
                Some(pb::agent_client_message::Message::KvClientMessage(
                    pb::KvClientMessage {
                        id: kv.id,
                        message: Some(reply),
                    },
                ))
            }
            Some(pb::agent_server_message::Message::ExecServerMessage(exec)) => {
                let result = match exec.message.as_ref().unwrap() {
                    pb::exec_server_message::Message::ReadArgs(args) => {
                        pb::exec_client_message::Message::ReadResult(pb::ReadResult {
                            result: Some(pb::read_result::Result::Success(pb::ReadSuccess {
                                path: args.path.clone(),
                                output: Some(pb::read_success::Output::Content(
                                    "# Existing plan\nbefore\n".into(),
                                )),
                                total_lines: 2,
                                ..Default::default()
                            })),
                        })
                    }
                    pb::exec_server_message::Message::WriteArgs(args) => {
                        pb::exec_client_message::Message::WriteResult(pb::WriteResult {
                            result: Some(pb::write_result::Result::Success(pb::WriteSuccess {
                                path: args.path.clone(),
                                ..Default::default()
                            })),
                        })
                    }
                    other => panic!("unexpected execution: {other:?}"),
                };
                let id = exec.id;
                execs.push(exec);
                Some(pb::agent_client_message::Message::ExecClientMessage(
                    pb::ExecClientMessage {
                        id,
                        message: Some(result),
                        ..Default::default()
                    },
                ))
            }
            Some(pb::agent_server_message::Message::InteractionQuery(query)) => {
                assert_eq!(
                    tool, "CreatePlan",
                    "editing an existing plan must not create another plan"
                );
                let Some(pb::interaction_query::Query::CreatePlanRequestQuery(create)) =
                    query.query
                else {
                    panic!("CreatePlan query")
                };
                assert_eq!(
                    create.args.unwrap().name,
                    arguments
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                );
                Some(pb::agent_client_message::Message::InteractionResponse(
                    pb::InteractionResponse {
                        id: query.id,
                        result: Some(pb::interaction_response::Result::CreatePlanRequestResponse(
                            pb::CreatePlanRequestResponse {
                                result: Some(pb::CreatePlanResult {
                                    plan_uri: PLAN_URI.into(),
                                    result: Some(pb::create_plan_result::Result::Success(
                                        pb::CreatePlanSuccess {},
                                    )),
                                }),
                            },
                        )),
                    },
                ))
            }
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(checkpoint)) => {
                state = Some(checkpoint);
                None
            }
            Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                if let Some(pb::interaction_update::Message::ToolCallCompleted(completed)) =
                    update.message
                {
                    cards.push(completed.tool_call.unwrap());
                }
                None
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
    (execs, provider.requests(), cards, state.unwrap())
}

#[tokio::test]
async fn existing_plan_write_keeps_its_path_and_sets_plan_mode_on_the_wire() {
    let path = "/workspace/.cursor/plans/existing.plan.md";
    let (execs, requests) = edit(pb::AgentMode::Plan, path).await;
    assert_eq!(execs.len(), 2);
    let Some(pb::exec_server_message::Message::WriteArgs(write)) = execs[1].message.as_ref() else {
        panic!("write")
    };
    assert_eq!(write.path, path);
    assert_eq!(write.file_text, "# Existing plan\nafter\n");
    assert!(
        WriteContract::decode(write.encode_to_vec().as_slice())
            .unwrap()
            .plan_mode,
        "WriteArgs lost Plan mode field 7"
    );
    assert!(requests[1]
        .history
        .iter()
        .any(|m| matches!(&m.content, ProjectedContent::ToolResult(r) if !r.is_error)));
}

#[tokio::test]
async fn plan_rejects_non_document_edits_before_any_client_execution() {
    let (execs, requests) = edit(pb::AgentMode::Plan, "/workspace/src/main.rs").await;
    assert!(
        execs.is_empty(),
        "Plan dispatched an edit outside its allowed document types"
    );
    assert!(requests[1]
        .history
        .iter()
        .any(|m| matches!(&m.content, ProjectedContent::ToolResult(r) if r.is_error)));
}

#[tokio::test]
async fn agent_mode_still_edits_source_without_plan_mode() {
    let (execs, _) = edit(pb::AgentMode::Agent, "/workspace/src/main.rs").await;
    let Some(pb::exec_server_message::Message::WriteArgs(write)) =
        execs.last().unwrap().message.as_ref()
    else {
        panic!("write")
    };
    assert!(
        !WriteContract::decode(write.encode_to_vec().as_slice())
            .unwrap()
            .plan_mode
    );
}

#[tokio::test]
async fn write_and_notebook_rejections_use_edit_cards_without_execution() {
    for (name, arguments) in [
        (
            "Write",
            json!({"path":"/workspace/src/main.rs", "contents":"changed"}),
        ),
        (
            "EditNotebook",
            json!({"target_notebook":"/workspace/analysis.ipynb", "cell_idx":0,"cell_language":"python","new_cell_source":"changed","is_new_cell":false}),
        ),
    ] {
        let (_directory, store) = fixtures::temp_store().await;
        let (execs, _, cards, _) = exchange(
            store,
            FakeProvider::default(),
            request(pb::AgentMode::Plan, "user"),
            "rejected",
            name,
            arguments,
        )
        .await;
        assert!(execs.is_empty());
        assert_eq!(cards.len(), 1);
        let Some(pb::tool_call::Tool::EditToolCall(card)) = &cards[0].tool else {
            panic!("expected Edit card")
        };
        assert!(matches!(
            card.result.as_ref().unwrap().result,
            Some(pb::edit_result::Result::Error(_))
        ));
    }
}

#[tokio::test]
async fn plan_write_preserves_a_remote_windows_file_uri() {
    let (_directory, store) = fixtures::temp_store().await;
    let path = "file:///C:/Users/test/.cursor/plans/existing%20plan.md";
    let (execs, _, _, _) = exchange(
        store,
        FakeProvider::default(),
        request(pb::AgentMode::Plan, "user"),
        "write",
        "Write",
        json!({"path":path,"contents":"# Revised plan\n"}),
    )
    .await;
    let Some(pb::exec_server_message::Message::WriteArgs(write)) =
        execs.last().unwrap().message.as_ref()
    else {
        panic!("write")
    };
    assert_eq!(write.path, path);
    assert_eq!(write.file_text, "# Revised plan\n");
    assert!(write.plan_mode);
}

#[tokio::test]
async fn resume_preserves_plan_policy_with_or_without_the_client_checkpoint() {
    for include_state in [true, false] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = FakeProvider::default();
        let (_, _, _, state) = exchange(
            store.clone(),
            provider.clone(),
            request(pb::AgentMode::Plan, "initial-user"),
            "initial",
            "StrReplace",
            json!({"path":"/workspace/plan.md","old_string":"before","new_string":"after"}),
        )
        .await;
        assert_eq!(state.mode, Some(pb::AgentMode::Plan as i32));
        let mut resumed = request(pb::AgentMode::Unspecified, "unused");
        resumed.conversation_state = include_state.then_some(state);
        resumed.action = Some(pb::ConversationAction {
            action: Some(pb::conversation_action::Action::ResumeAction(
                pb::ResumeAction::default(),
            )),
            ..Default::default()
        });
        let (execs, requests, _, state) = exchange(
            store,
            provider,
            resumed,
            "resumed",
            "StrReplace",
            json!({"path":"/workspace/main.rs","old_string":"before","new_string":"after"}),
        )
        .await;
        assert!(
            execs.is_empty(),
            "Plan restriction disappeared: include_state={include_state}"
        );
        assert_eq!(state.mode, Some(pb::AgentMode::Plan as i32));
        assert_eq!(requests[1].prompt, requests[2].prompt);
        if include_state {
            let content = |request: &cursor_server::model::ModelRequest| {
                request
                    .history
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

#[tokio::test]
async fn created_plan_is_revised_at_its_returned_uri_on_the_next_turn() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = FakeProvider::default();
    let (execs, requests, _, state) = exchange(
        store.clone(),
        provider.clone(),
        request(pb::AgentMode::Plan, "create-user"),
        "create",
        "CreatePlan",
        json!({"plan":"# Existing plan\nbefore\n", "overview":"Plan", "todos":[]}),
    )
    .await;
    assert!(execs.is_empty());
    assert!(requests[1].history.iter().any(|m| matches!(&m.content, ProjectedContent::ToolResult(r) if r.content.contains(PLAN_URI) && r.content.contains("each call creates a new file"))));
    let mut next = request(pb::AgentMode::Plan, "revise-user");
    next.conversation_state = Some(state);
    let (execs, requests, _, _) = exchange(
        store,
        provider,
        next,
        "revise",
        "StrReplace",
        json!({"path":PLAN_URI,"old_string":"before","new_string":"after"}),
    )
    .await;
    assert_eq!(execs.len(), 2);
    let Some(pb::exec_server_message::Message::WriteArgs(write)) = execs[1].message.as_ref() else {
        panic!("write")
    };
    assert_eq!(write.path, PLAN_URI);
    assert!(write.plan_mode);
    assert_eq!(write.file_text, "# Existing plan\nafter\n");
    assert_eq!(requests[1].prompt, requests[2].prompt);
    let content = |request: &cursor_server::model::ModelRequest| {
        request
            .history
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect::<Vec<_>>()
    };
    assert!(content(&requests[2]).starts_with(&content(&requests[1])));
    assert!(requests[3].history.starts_with(&requests[2].history));
}

#[tokio::test]
async fn explicit_agent_mode_or_plan_approval_allows_implementation_edits() {
    for approve in [false, true] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = FakeProvider::default();
        exchange(
            store.clone(),
            provider.clone(),
            request(pb::AgentMode::Plan, "plan-user"),
            "plan",
            "Write",
            json!({"path":"/workspace/plan.md","contents":"# Plan"}),
        )
        .await;
        let mut next = request(pb::AgentMode::Agent, "agent-user");
        if approve {
            next.action = Some(pb::ConversationAction {
                action: Some(pb::conversation_action::Action::ExecutePlanAction(
                    pb::ExecutePlanAction {
                        plan_file_content: Some("# Approved plan".into()),
                        plan_file_uri: Some(PLAN_URI.into()),
                        execution_mode: pb::AgentMode::Agent as i32,
                        ..Default::default()
                    },
                )),
                ..Default::default()
            });
        }
        let (execs, _, _, state) = exchange(
            store,
            provider,
            next,
            "implement",
            "Write",
            json!({"path":"/workspace/main.rs","contents":"implementation"}),
        )
        .await;
        let Some(pb::exec_server_message::Message::WriteArgs(write)) =
            execs.last().unwrap().message.as_ref()
        else {
            panic!("write")
        };
        assert!(!write.plan_mode);
        assert_eq!(state.mode, Some(pb::AgentMode::Agent as i32));
    }
}

#[tokio::test]
async fn relative_canvas_edit_uses_the_request_client_directory() {
    let (_directory, store) = fixtures::temp_store().await;
    let mut req = request(pb::AgentMode::Plan, "remote-user");
    let Some(pb::conversation_action::Action::UserMessageAction(user)) =
        req.action.as_mut().unwrap().action.as_mut()
    else {
        panic!("user")
    };
    user.request_context = Some(pb::RequestContext {
        env: Some(pb::RequestContextEnv {
            os_version: "Windows 11".into(),
            process_working_directory: Some(r"D:\remote\.cursor\projects\p\canvases".into()),
            ..Default::default()
        }),
        ..Default::default()
    });
    let (execs, _, _, _) = exchange(
        store,
        FakeProvider::default(),
        req,
        "remote",
        "Write",
        json!({"path":"board.canvas.tsx","contents":"canvas"}),
    )
    .await;
    let Some(pb::exec_server_message::Message::WriteArgs(write)) =
        execs.last().unwrap().message.as_ref()
    else {
        panic!("write")
    };
    assert_eq!(write.path, "board.canvas.tsx");
    assert!(write.plan_mode);
}
