//! Exercise the final-payload/notification ordering without a provider timing race.
use super::*;
use crate::cursor::{
    checkpoint::subagents,
    prompting::PromptAssets,
    protocol::{connect, proto::agent::v1 as pb},
    transport::TransportRegistry,
};
use crate::{model::ModelInvocation, provider::ProviderStream};
use prost::Message;

struct NoProvider;
impl Provider for NoProvider {
    fn stream(&self, _: ModelInvocation, _: tokio_util::sync::CancellationToken) -> ProviderStream {
        panic!("status publication must not call the model")
    }
}

#[tokio::test]
async fn notification_after_final_payload_is_flushed_before_releasing_the_run() {
    late_notification(None).await;
}

#[tokio::test]
async fn notification_is_flushed_when_parent_is_cancelled() {
    late_notification(Some(super::super::TransportFinish::Cancelled)).await;
}

#[tokio::test]
async fn notification_is_flushed_when_parent_fails() {
    late_notification(Some(super::super::TransportFinish::Failed(
        crate::Error::Protocol("fixture".into()),
    )))
    .await;
}

async fn late_notification(interrupted: Option<super::super::TransportFinish>) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let compiler = PromptCompiler::new(PromptAssets::embedded().unwrap());
    let registry = ConversationRegistry::new(
        store.clone(),
        Arc::new(NoProvider),
        compiler.clone(),
        WebCache::default(),
        None,
    );
    let transports = TransportRegistry::new(store.clone(), Arc::new(NoProvider), compiler);
    let handle = transports.get_or_create("parent").await.unwrap();
    let mut output = handle.subscribe();
    let conversation = ConversationId::new("conversation");
    let checkpoint = store.ensure_conversation(&conversation).await.unwrap();
    let run_id = RunId::new("run");
    let (_port, _core, run_handle) = crate::run::channel(run_id.clone(), 4);
    assert!(
        registry
            .activate(conversation.clone(), run_id.clone(), run_handle, true)
            .await
    );
    // The final checkpoint is already built; no further worker writes remain.
    let mut state = pb::ConversationStateStructure::default();
    state.subagent_runs_by_parent_tool_call_id.insert(
        "task".into(),
        pb::SubagentRunState {
            parent_tool_call_id: "task".into(),
            status: pb::SubagentRunStatus::Backgrounded as i32,
            ..Default::default()
        },
    );
    subagents::save(&store, checkpoint, &mut state)
        .await
        .unwrap();
    // Cancellation must retain the presentation already sent to the client,
    // including partial tools that have not become a settled DB checkpoint.
    state.pending_tool_calls = vec!["streamed partial tool".into()];
    handle
        .emit(&pb::AgentServerMessage {
            message: Some(
                pb::agent_server_message::Message::ConversationCheckpointUpdate(state.clone()),
            ),
            ..Default::default()
        })
        .unwrap();
    output.try_recv().unwrap();
    let mut finish = match interrupted {
        Some(finish) => super::super::RunFinish::Transport(finish),
        None => super::super::RunFinish::TurnCompleted(Box::new(state)),
    };
    let mut notification = pb::AgentRunRequest {
        conversation_id: Some(conversation.to_string()),
        action: Some(pb::ConversationAction {
            action: Some(
                pb::conversation_action::Action::BackgroundTaskCompletionAction(
                    pb::BackgroundTaskCompletionAction {
                        completions: vec![pb::BackgroundTaskCompletion {
                            kind: pb::BackgroundTaskKind::Subagent as i32,
                            reason: pb::BackgroundTaskCompletionReason::TaskFinished as i32,
                            status: pb::BackgroundTaskStatus::Success as i32,
                            task_id: "worker".into(),
                            tool_call_id: Some("task".into()),
                            completed_at_ms: Some(300),
                            record_only: true,
                            ..Default::default()
                        }],
                    },
                ),
            ),
            ..Default::default()
        }),
        ..Default::default()
    };
    subagents::record(&store, "side", &mut notification)
        .await
        .unwrap();
    registry
        .publish_idle_checkpoint(&handle, conversation.as_str())
        .await
        .unwrap();
    assert!(output.try_recv().is_err(), "active run owns publication");
    registry
        .release(&conversation, &run_id, &handle, &mut finish)
        .await
        .unwrap();
    let frame = output
        .try_recv()
        .expect("release must flush the late status");
    let (_, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
    let message = pb::AgentServerMessage::decode(payload).unwrap();
    let Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) =
        message.message
    else {
        panic!("expected checkpoint")
    };
    assert_eq!(
        state.subagent_runs_by_parent_tool_call_id["task"].status,
        pb::SubagentRunStatus::Success as i32
    );
    assert_eq!(state.pending_tool_calls, vec!["streamed partial tool"]);
    if let super::super::RunFinish::TurnCompleted(final_state) = finish {
        assert_eq!(*final_state, state);
    }
    assert!(!registry
        .inner
        .current
        .lock()
        .await
        .contains_key(&conversation));
    transports.shutdown().await;
}
