//! Verifies selected-model context updates and the existing Cursor setting priority.
use super::{break_messages::compile_request_context, model::subagent_model_context};
use crate::{
    cursor::{
        prompting::{Mode, PromptAssets, PromptCompiler},
        protocol::proto::agent::v1 as pb,
        tools::runtime::{ExecContext, SubagentModel},
    },
    model::{project_messages, CanonicalMessage, ModelSpec, Origin, Role, ToolCall},
};

fn request(ids: &[&str]) -> pb::AgentRunRequest {
    pb::AgentRunRequest {
        selected_subagent_models: ids
            .iter()
            .map(|id| pb::RequestedModel {
                model_id: (*id).into(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn selected_models_are_sorted_deduplicated_and_never_include_credentials() {
    let mut a = request(&["z-custom", "a-custom", "z-custom", ""]);
    a.selected_subagent_model_details.push(pb::ModelDetails {
        model_id: "a-custom".into(),
        display_name: "A <custom>".into(),
        credentials: Some(pb::model_details::Credentials::ApiKeyCredentials(
            pb::ApiKeyCredentials {
                api_key: "do-not-leak".into(),
                ..Default::default()
            },
        )),
        ..Default::default()
    });
    let mut b = a.clone();
    b.selected_subagent_models.reverse();
    let text = subagent_model_context(&a);
    assert_eq!(text, subagent_model_context(&b));
    assert!(text.contains("A &lt;custom&gt;"));
    assert!(!text.contains("do-not-leak"));
    assert_eq!(text.matches("z-custom").count(), 1);
    assert!(text.find("a-custom").unwrap() < text.find("z-custom").unwrap());
}

#[test]
fn selected_model_changes_append_context_without_rewriting_history_or_tools() {
    let context = pb::RequestContext::default();
    let a = request(&["custom-a"]);
    let b = request(&["custom-b"]);
    let mut history = vec![];
    let compiler = PromptCompiler::new(PromptAssets::embedded().unwrap());
    let prompt = compiler
        .prompt_spec(Mode::Agent, &ModelSpec::new("parent"), &[], false)
        .unwrap();
    for (event, current) in [("a1", &a), ("b1", &b), ("a2", &a), ("empty", &request(&[]))] {
        let before = project_messages(&history).unwrap();
        let message = compile_request_context(event, current, &context, &history)
            .unwrap()
            .unwrap();
        assert_eq!(message.message_id, format!("request-context:{event}"));
        history.push(message);
        history.push(CanonicalMessage::text(
            format!("runtime:{event}"),
            Role::User,
            Origin::User,
            "continue",
        ));
        let after = project_messages(&history).unwrap();
        assert_eq!(before, after[..before.len()]);
        assert!(
            compile_request_context("retry", current, &context, &history)
                .unwrap()
                .is_none()
        );
        let next_prompt = compiler
            .prompt_spec(Mode::Agent, &ModelSpec::new("parent"), &[], false)
            .unwrap();
        assert_eq!(prompt, next_prompt);
    }
    assert_eq!(
        history
            .iter()
            .filter(|m| m.message_id.starts_with("request-context:"))
            .count(),
        4
    );
}

#[test]
fn context_reports_the_effective_override_and_empty_list_is_not_unavailability() {
    use pb::subagent_model_override::Selection;
    for (selection, expected) in [
        (
            Selection::Model(pb::RequestedModel {
                model_id: "chosen-custom".into(),
                ..Default::default()
            }),
            "chosen-custom",
        ),
        (
            Selection::Model(pb::RequestedModel {
                model_id: "default".into(),
                ..Default::default()
            }),
            "inherit_parent",
        ),
        (Selection::Inherit(true), "inherit_parent"),
        (Selection::Disabled(true), "disabled"),
    ] {
        let mut req = request(&[]);
        req.subagent_model_overrides
            .push(pb::SubagentModelOverride {
                subagent_type: "generalPurpose".into(),
                selection: Some(selection),
            });
        let text = subagent_model_context(&req);
        assert!(text.contains(expected));
        assert!(text.contains("not that subagents are unavailable"));
    }
}

#[test]
fn task_execution_still_prioritizes_cursor_setting_over_explicit_model() {
    let mut context = ExecContext {
        conversation_id: "conversation".into(),
        root_conversation_id: "conversation".into(),
        default_subagent_model: "parent".into(),
        subagent_model: Some(SubagentModel::Model("chosen-custom".into())),
        allow_subagents: true,
        subagents_disabled: false,
        terminals_folder: String::new(),
        admin_command_denylist: vec![],
        mcp_routes: Default::default(),
    };
    let call = ToolCall {
        index: 0,
        arguments_text: "{}".into(),
        argument_error: None,
        call_id: "task".into(),
        model_call_id: "call".into(),
        name: "Task".into(),
        arguments: serde_json::json!({"model":"client-listed-custom", "description":"test", "prompt":"test"}),
    };
    assert_eq!(
        context.prepare_call(&call).unwrap().arguments["model"],
        "chosen-custom"
    );
    context.subagent_model = None;
    assert_eq!(
        context.prepare_call(&call).unwrap().arguments["model"],
        "client-listed-custom"
    );
    let mut inherited = call;
    inherited.arguments["model"] = serde_json::json!("inherit");
    assert_eq!(
        context.prepare_call(&inherited).unwrap().arguments["model"],
        "parent"
    );
}

#[test]
fn model_list_survives_checkpoint_replay_without_duplicate_context() {
    use crate::cursor::checkpoint::messages::{decode, stable_messages};
    let req = request(&["custom-checkpoint-model"]);
    let context = pb::RequestContext::default();
    let message = compile_request_context("turn1", &req, &context, &[])
        .unwrap()
        .unwrap();
    let wire = stable_messages("", std::slice::from_ref(&message), "parent").unwrap();
    let recovered = decode(&wire[0], "checkpoint-blob".into()).unwrap();
    assert_eq!(recovered.message_id, message.message_id);
    assert_eq!(recovered.content, message.content);
    assert!(
        compile_request_context("turn2", &req, &context, &[recovered])
            .unwrap()
            .is_none()
    );
}
