use super::*;

fn notice() -> pb::BackgroundTaskCompletion {
    pb::BackgroundTaskCompletion {
        kind: pb::BackgroundTaskKind::Subagent as i32,
        task_id: "worker".into(),
        tool_call_id: Some("task".into()),
        status: pb::BackgroundTaskStatus::Success as i32,
        reason: pb::BackgroundTaskCompletionReason::TaskFinished as i32,
        completed_at_ms: Some(200),
        ..Default::default()
    }
}

#[test]
fn progress_messages_and_unidentified_notifications_do_not_finish_workers() {
    for reason in [
        pb::BackgroundTaskCompletionReason::TaskProgress,
        pb::BackgroundTaskCompletionReason::WorkerMessage,
    ] {
        let mut state = pb::ConversationStateStructure::default();
        let mut completion = notice();
        completion.reason = reason as i32;
        apply(&mut state, &completion, 999).unwrap();
        assert!(state.subagent_runs_by_parent_tool_call_id.is_empty());
    }
    let mut completion = notice();
    completion.tool_call_id = Some("  ".into());
    let mut state = pb::ConversationStateStructure::default();
    apply(&mut state, &completion, 999).unwrap();
    assert!(state.subagent_runs_by_parent_tool_call_id.is_empty());
}

#[test]
fn attention_preserves_status_and_metadata_until_an_explicit_terminal_result() {
    let mut state = pb::ConversationStateStructure::default();
    state.subagent_runs_by_parent_tool_call_id.insert(
        "task".into(),
        pb::SubagentRunState {
            status: pb::SubagentRunStatus::Backgrounded as i32,
            title: Some("Original title".into()),
            transcript_path: Some("/tmp/transcript".into()),
            environment: pb::SubagentExecutionEnvironment::Local as i32,
            ..Default::default()
        },
    );
    let mut completion = notice();
    completion.status = pb::BackgroundTaskStatus::Unspecified as i32;
    completion.reason = pb::BackgroundTaskCompletionReason::WorkerNeedsAttention as i32;
    completion.title = "  ".into();
    completion.detail = Some("  Waiting for approval  ".into());
    apply(&mut state, &completion, 999).unwrap();
    let run = &state.subagent_runs_by_parent_tool_call_id["task"];
    assert_eq!(run.status, pb::SubagentRunStatus::Backgrounded as i32);
    assert_eq!(run.title.as_deref(), Some("Original title"));
    assert_eq!(run.detail.as_deref(), Some("Waiting for approval"));
    assert_eq!(run.transcript_path.as_deref(), Some("/tmp/transcript"));
    assert_eq!(
        run.environment,
        pb::SubagentExecutionEnvironment::Local as i32
    );
    completion.status = pb::BackgroundTaskStatus::Aborted as i32;
    completion.reason = pb::BackgroundTaskCompletionReason::TaskFinished as i32;
    completion.completed_at_ms = Some(300);
    apply(&mut state, &completion, 999).unwrap();
    assert_eq!(
        state.subagent_runs_by_parent_tool_call_id["task"].status,
        pb::SubagentRunStatus::Aborted as i32
    );
}

#[test]
fn equal_timestamp_can_update_a_terminal_result_but_older_notifications_cannot() {
    let mut state = pb::ConversationStateStructure::default();
    let mut completion = notice();
    apply(&mut state, &completion, 999).unwrap();
    completion.status = pb::BackgroundTaskStatus::Error as i32;
    completion.completed_at_ms = Some(100);
    apply(&mut state, &completion, 999).unwrap();
    assert_eq!(
        state.subagent_runs_by_parent_tool_call_id["task"].status,
        pb::SubagentRunStatus::Success as i32
    );
    completion.completed_at_ms = Some(200);
    apply(&mut state, &completion, 999).unwrap();
    assert_eq!(
        state.subagent_runs_by_parent_tool_call_id["task"].status,
        pb::SubagentRunStatus::Error as i32
    );
}

#[tokio::test]
async fn malformed_record_only_batch_rolls_back_all_status_changes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let mut malformed = notice();
    malformed.record_only = true;
    malformed.status = 999;
    let mut valid = notice();
    valid.record_only = true;
    let mut request = pb::AgentRunRequest {
        conversation_id: Some("invalid-batch".into()),
        action: Some(pb::ConversationAction {
            action: Some(
                pb::conversation_action::Action::BackgroundTaskCompletionAction(
                    pb::BackgroundTaskCompletionAction {
                        completions: vec![valid, malformed],
                    },
                ),
            ),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(record(&store, "request", &mut request).await.is_err());
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM conversations), (SELECT count(*) FROM cursor_checkpoints), (SELECT count(*) FROM cursor_subagent_runs)").fetch_one(store.pool()).await.unwrap();
    assert_eq!(counts, (0, 0, 0));
}
