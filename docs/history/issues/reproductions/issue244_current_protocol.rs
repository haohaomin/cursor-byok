//! Diagnostic probe for Issue #244; copy to server/tests/issue244_probe.rs to run.
//! This records current behavior, including rejection of obsolete mode 6.
//! It is not a permanent requirement to reject future Project implementations.
#[path = "support/fake_provider.rs"]
mod fake_provider;
#[path = "support/fixtures.rs"]
mod fixtures;

use std::{collections::HashMap, sync::Arc, time::Duration};

use cursor_server::{
    cursor::{
        prompting::{PromptAssets, PromptCompiler},
        protocol::{connect, proto::agent::v1 as pb},
        tools::{codec, runtime::ExecContext},
        TransportCommand, TransportRegistry,
    },
    model::ToolCall,
    provider::{FinishReason, ModelEvent},
};
use prost::Message;
use serde_json::json;

#[test]
fn current_task_new_and_resume_preserve_background_assignment() {
    let context = ExecContext {
        conversation_id: "parent-conversation".into(),
        root_conversation_id: "root-conversation".into(),
        agent_session_id: None,
        default_subagent_model: "model".into(),
        subagent_model: None,
        terminals_folder: "/tmp/issue244-terminals".into(),
        workspace_paths: Vec::new(),
        admin_command_denylist: Vec::new(),
        allow_subagents: true,
        subagents_disabled: false,
        mcp_routes: HashMap::new(),
    };
    for resume in [None, Some("existing-child")] {
        let mut arguments = json!({
            "description": "Inspect source",
            "prompt": "Read the source and report the exact failure",
            "model": "configured-model",
            "subagent_type": "generalPurpose",
            "run_in_background": true,
            "interrupt": true
        });
        if let Some(agent_id) = resume {
            arguments["resume"] = json!(agent_id);
        }
        let call = ToolCall {
            index: 0,
            call_id: if resume.is_some() {
                "resume-call"
            } else {
                "new-call"
            }
            .into(),
            model_call_id: "model-round".into(),
            name: "Task".into(),
            arguments_text: arguments.to_string(),
            arguments,
            argument_error: None,
        };
        let message = codec::request(1, &call, &context).unwrap();
        let decoded = pb::AgentServerMessage::decode(message.encode_to_vec().as_slice()).unwrap();
        let Some(pb::agent_server_message::Message::ExecServerMessage(exec)) = decoded.message
        else {
            panic!("expected ExecServerMessage");
        };
        let Some(pb::exec_server_message::Message::SubagentArgs(args)) = exec.message else {
            panic!(
                "Task must create or resume a subagent, not force an existing call to background"
            );
        };
        assert_eq!(args.tool_call_id, call.call_id);
        assert_eq!(args.prompt, "Read the source and report the exact failure");
        assert_eq!(args.model_id, "configured-model");
        assert_eq!(args.resume_agent_id.as_deref(), resume);
        assert_eq!(args.run_in_background, Some(true));
        assert_eq!(args.interrupt, Some(true));
        assert_eq!(
            args.parent_conversation_id.as_deref(),
            Some("parent-conversation")
        );
        assert_eq!(
            args.root_parent_conversation_id.as_deref(),
            Some("root-conversation")
        );
        println!("Task resume={resume:?}: SubagentArgs carries assignment and background flag");
    }
}

#[tokio::test]
async fn legacy_project_mode_is_rejected_before_provider_execution() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "unexpected".into(),
        },
        ModelEvent::Done(FinishReason::Stop),
    ]);
    let registry = TransportRegistry::new(
        store,
        Arc::new(provider.clone()),
        PromptCompiler::new(PromptAssets::embedded().unwrap()),
    );
    let handle = registry
        .get_or_create("issue244-project-probe")
        .await
        .unwrap();
    let mut output = handle.subscribe();
    handle
        .command(TransportCommand::Append {
            seqno: 0,
            message: Box::new(pb::AgentClientMessage {
                message: Some(pb::agent_client_message::Message::RunRequest(
                    pb::AgentRunRequest {
                        conversation_id: Some("issue244-project-probe".into()),
                        run_id: Some("issue244-project-probe".into()),
                        requested_model: Some(pb::RequestedModel {
                            model_id: "test-model".into(),
                            ..Default::default()
                        }),
                        action: Some(pb::ConversationAction {
                            action: Some(pb::conversation_action::Action::UserMessageAction(
                                pb::UserMessageAction {
                                    user_message: Some(pb::UserMessage {
                                        text: "Continue the project".into(),
                                        message_id: "issue244-project-user".into(),
                                        mode: 6,
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
            }),
        })
        .await
        .unwrap();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            let terminal: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            println!("mode=6 terminal={terminal}");
            assert!(terminal["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unsupported Cursor agent mode: AGENT_MODE_PROJECT"));
            break;
        }
    }
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}
