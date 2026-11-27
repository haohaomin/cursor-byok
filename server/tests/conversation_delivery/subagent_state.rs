//! Completion status is presentation state, independent of model wakeups.
use super::*;

fn running_state() -> pb::ConversationStateStructure {
    pb::ConversationStateStructure {
        mode: Some(pb::AgentMode::Agent as i32),
        subagent_runs_by_parent_tool_call_id: HashMap::from([(
            "task-call".into(),
            pb::SubagentRunState {
                parent_tool_call_id: "task-call".into(),
                subagent_id: Some("worker".into()),
                status: pb::SubagentRunStatus::Backgrounded as i32,
                environment: pb::SubagentExecutionEnvironment::Local as i32,
                transcript_path: Some("/tmp/worker-transcript.jsonl".into()),
                ..Default::default()
            },
        )]),
        ..Default::default()
    }
}

fn notice(
    state: pb::ConversationStateStructure,
    record_only: bool,
    timestamp: u64,
    status: pb::BackgroundTaskStatus,
) -> pb::AgentClientMessage {
    let mut message = completion_run_with_detail("worker", "completion", state, "Worker finished.");
    let Some(pb::agent_client_message::Message::RunRequest(run)) = message.message.as_mut() else {
        unreachable!()
    };
    let Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(action)) =
        run.action.as_mut().unwrap().action.as_mut()
    else {
        unreachable!()
    };
    let completion = &mut action.completions[0];
    completion.record_only = record_only;
    completion.completed_at_ms = Some(timestamp);
    completion.status = status as i32;
    message
}

fn registry(
    store: cursor_server::store::Store,
    provider: &fake_provider::FakeProvider,
) -> TransportRegistry {
    TransportRegistry::new(
        store,
        Arc::new(provider.clone()),
        PromptCompiler::new(PromptAssets::embedded().unwrap()),
    )
}

fn completion(message: &mut pb::AgentClientMessage) -> &mut pb::BackgroundTaskCompletion {
    let Some(pb::agent_client_message::Message::RunRequest(run)) = message.message.as_mut() else {
        unreachable!()
    };
    let Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(action)) =
        run.action.as_mut().unwrap().action.as_mut()
    else {
        unreachable!()
    };
    &mut action.completions[0]
}

fn user_turn(
    state: pb::ConversationStateStructure,
    id: &str,
    text: &str,
) -> pb::AgentClientMessage {
    let mut message = notice(state, false, 200, pb::BackgroundTaskStatus::Success);
    let Some(pb::agent_client_message::Message::RunRequest(run)) = message.message.as_mut() else {
        unreachable!()
    };
    run.action = Some(pb::ConversationAction {
        action: Some(pb::conversation_action::Action::UserMessageAction(
            pb::UserMessageAction {
                user_message: Some(pb::UserMessage {
                    text: text.into(),
                    message_id: id.into(),
                    mode: pb::AgentMode::Agent as i32,
                    ..Default::default()
                }),
                request_context: Some(pb::RequestContext::default()),
                ..Default::default()
            },
        )),
        ..Default::default()
    });
    message
}

fn final_state(messages: &[pb::AgentServerMessage]) -> pb::ConversationStateStructure {
    messages
        .iter()
        .rev()
        .find_map(|message| match &message.message {
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) => {
                Some(state.clone())
            }
            _ => None,
        })
        .expect("state-only completion must publish its updated checkpoint")
}

fn assert_finished(
    state: &pb::ConversationStateStructure,
    status: pb::SubagentRunStatus,
    timestamp: u64,
) {
    let run = &state.subagent_runs_by_parent_tool_call_id["task-call"];
    assert_eq!(run.status, status as i32);
    assert_eq!(run.completed_timestamp_ms, Some(timestamp));
    assert_eq!(
        run.completion_reason,
        Some(pb::BackgroundTaskCompletionReason::TaskFinished as i32)
    );
    assert_eq!(
        run.environment,
        pb::SubagentExecutionEnvironment::Local as i32
    );
    assert_eq!(
        run.transcript_path.as_deref(),
        Some("/tmp/worker-transcript.jsonl")
    );
}

#[tokio::test]
async fn visible_completion_publishes_the_finished_subagent_state() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(stop_response("reply", "Done."));
    let registry = registry(store, &provider);
    let handle = registry.get_or_create("visible-state").await.unwrap();
    let (state, _) = drive_completion(
        &handle,
        notice(
            running_state(),
            false,
            200,
            pb::BackgroundTaskStatus::Success,
        ),
    )
    .await;
    assert_finished(&state, pb::SubagentRunStatus::Success, 200);
    assert_eq!(provider.requests().len(), 1);
    registry.shutdown().await;
}

#[tokio::test]
async fn record_only_persists_state_without_creating_a_model_run() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(store.clone(), &provider);
    let handle = registry.get_or_create("record-state").await.unwrap();
    let messages = drive_completion_end(
        &handle,
        notice(
            running_state(),
            true,
            200,
            pb::BackgroundTaskStatus::Success,
        ),
        false,
    )
    .await;
    let state = final_state(&messages);
    assert_finished(&state, pb::SubagentRunStatus::Success, 200);
    assert!(provider.requests().is_empty());
    let counts: (i64, i64) =
        sqlx::query_as("SELECT (SELECT COUNT(*) FROM runs), (SELECT COUNT(*) FROM messages)")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(
        counts,
        (0, 0),
        "presentation updates must not create model history"
    );
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT state FROM cursor_checkpoints ORDER BY checkpoint_id DESC LIMIT 1",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        pb::ConversationStateStructure::decode(bytes.as_slice()).unwrap(),
        state
    );
    registry.shutdown().await;
}

#[tokio::test]
async fn late_completion_after_restart_preserves_the_newer_state() {
    let (directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let initial = registry(store.clone(), &provider);
    let handle = initial.get_or_create("newer-completion").await.unwrap();
    drive_completion_end(
        &handle,
        notice(
            running_state(),
            true,
            300,
            pb::BackgroundTaskStatus::Success,
        ),
        false,
    )
    .await;
    initial.shutdown().await;
    store.pool().close().await;
    let reopened = cursor_server::store::Store::connect(&format!(
        "sqlite://{}",
        directory.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let restarted = registry(reopened, &provider);
    let handle = restarted.get_or_create("late-completion").await.unwrap();
    let messages = drive_completion_end(
        &handle,
        notice(running_state(), true, 100, pb::BackgroundTaskStatus::Error),
        false,
    )
    .await;
    assert_finished(&final_state(&messages), pb::SubagentRunStatus::Success, 300);
    assert!(provider.requests().is_empty());
    restarted.shutdown().await;
}

#[tokio::test]
async fn status_only_completion_during_a_run_survives_its_stale_checkpoint_writer() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let ready = provider.push_gated(stop_response("active", "Original work finished."));
    let registry = registry(store.clone(), &provider);
    let handle = registry.get_or_create("active-parent-state").await.unwrap();
    let parent = tokio::spawn(async move {
        drive_completion(
            &handle,
            user_turn(running_state(), "active-user", "Continue the parent work."),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while provider.requests().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let handle = registry.get_or_create("active-status-only").await.unwrap();
    let updates = drive_completion_end(
        &handle,
        notice(
            running_state(),
            true,
            300,
            pb::BackgroundTaskStatus::Success,
        ),
        false,
    )
    .await;
    assert_eq!(
        updates.len(),
        1,
        "the side transport must not overwrite a streaming parent's checkpoint"
    );
    ready.notify_one();
    let (state, _) = parent.await.unwrap();
    assert_finished(&state, pb::SubagentRunStatus::Success, 300);
    assert_eq!(provider.requests().len(), 1);
    let texts = serde_json::to_string(&provider.requests()[0].history).unwrap();
    assert!(texts.contains("Continue the parent work."));
    assert!(!texts.contains("Worker finished."));
    registry.shutdown().await;
}

#[tokio::test]
async fn state_notifications_preserve_compaction_and_later_user_history() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(stop_response("before", "OLD_RESPONSE"));
    provider.push(stop_response("summary", "LATEST_SUMMARY"));
    provider.push(stop_response("later", "LATER_RESPONSE"));
    provider.push(stop_response("after-record", "Kept state and history."));
    let registry = registry(store.clone(), &provider);
    let handle = registry.get_or_create("state-initial").await.unwrap();
    let (initial, _) =
        drive_completion(&handle, user_turn(running_state(), "start", "Start work.")).await;
    let handle = registry.get_or_create("state-summary").await.unwrap();
    let (compacted, _) = drive_completion(
        &handle,
        user_turn(initial.clone(), "summarize", "/summarize"),
    )
    .await;
    assert_eq!(compacted.self_summary_count, 1);
    let handle = registry.get_or_create("state-later").await.unwrap();
    let (later, _) = drive_completion(
        &handle,
        user_turn(compacted.clone(), "later", "A later user request."),
    )
    .await;
    let handle = registry.get_or_create("state-late-record").await.unwrap();
    let updates = drive_completion_end(
        &handle,
        notice(initial, true, 300, pb::BackgroundTaskStatus::Success),
        false,
    )
    .await;
    let state = final_state(&updates);
    assert_finished(&state, pb::SubagentRunStatus::Success, 300);
    assert_eq!(
        state.root_prompt_messages_json,
        later.root_prompt_messages_json
    );
    assert_eq!(state.turns, later.turns);
    assert_eq!(state.summary, compacted.summary);
    assert_eq!(state.self_summary_count, 1);
    assert_eq!(provider.requests().len(), 3);
    // A later client message can still carry the pre-notification checkpoint.
    let handle = registry.get_or_create("state-after-record").await.unwrap();
    let (state, _) = drive_completion(&handle, user_turn(later, "newest", "Continue.")).await;
    assert_finished(&state, pb::SubagentRunStatus::Success, 300);
    let requests = provider.requests();
    assert!(requests[3].history.starts_with(&requests[2].history));
    assert!(serde_json::to_string(&requests[3].history)
        .unwrap()
        .contains("LATER_RESPONSE"));
    registry.shutdown().await;
}

#[tokio::test]
async fn separate_store_connections_record_both_workers_atomically() {
    let (directory, first_store) = fixtures::temp_store().await;
    let second_store = cursor_server::store::Store::connect(&format!(
        "sqlite://{}",
        directory.path().join("test.db").display()
    ))
    .await
    .unwrap();
    let provider = fake_provider::FakeProvider::default();
    let first_registry = registry(first_store.clone(), &provider);
    let second_registry = registry(second_store, &provider);
    let first_handle = first_registry
        .get_or_create("concurrent-first")
        .await
        .unwrap();
    let second_handle = second_registry
        .get_or_create("concurrent-second")
        .await
        .unwrap();
    let first = notice(
        running_state(),
        true,
        300,
        pb::BackgroundTaskStatus::Success,
    );
    let mut second = first.clone();
    completion(&mut second).tool_call_id = Some("other-call".into());
    completion(&mut second).subagent_id = Some("other-worker".into());
    tokio::join!(
        drive_completion_end(&first_handle, first, false),
        drive_completion_end(&second_handle, second, false)
    );
    let bytes: Vec<u8> = sqlx::query_scalar(
        "SELECT state FROM cursor_checkpoints ORDER BY checkpoint_id DESC LIMIT 1",
    )
    .fetch_one(first_store.pool())
    .await
    .unwrap();
    let state = pb::ConversationStateStructure::decode(bytes.as_slice()).unwrap();
    assert_finished(&state, pb::SubagentRunStatus::Success, 300);
    assert_eq!(
        state.subagent_runs_by_parent_tool_call_id["other-call"].status,
        pb::SubagentRunStatus::Success as i32
    );
    assert!(provider.requests().is_empty());
    first_registry.shutdown().await;
    second_registry.shutdown().await;
}

#[tokio::test]
async fn blocked_goal_wakeup_keeps_finished_workers_without_calling_the_model() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(store, &provider);
    let mut old = running_state();
    old.goal_state = Some(pb::GoalState {
        conversation_id: "parent-conversation".into(),
        goal_id: uuid::Uuid::new_v4().to_string(),
        objective: "Finish the original work".into(),
        status: pb::GoalStatus::Active as i32,
        ..Default::default()
    });
    old.pending_tool_calls = vec!["pending approval".into()];
    let handle = registry.get_or_create("before-goal").await.unwrap();
    drive_completion_end(
        &handle,
        notice(old.clone(), true, 300, pb::BackgroundTaskStatus::Success),
        false,
    )
    .await;
    let mut message = user_turn(old.clone(), "goal-wakeup", "ignored");
    let Some(pb::agent_client_message::Message::RunRequest(run)) = message.message.as_mut() else {
        unreachable!()
    };
    run.action.as_mut().unwrap().action =
        Some(pb::conversation_action::Action::GoalContinuationAction(
            pb::GoalContinuationAction::default(),
        ));
    let handle = registry.get_or_create("blocked-goal").await.unwrap();
    let messages = drive_completion_end(&handle, message, false).await;
    let state = final_state(&messages);
    assert_finished(&state, pb::SubagentRunStatus::Success, 300);
    assert_eq!(state.goal_state, old.goal_state);
    assert_eq!(state.pending_tool_calls, old.pending_tool_calls);
    assert!(provider.requests().is_empty());
    registry.shutdown().await;
}

#[tokio::test]
async fn parent_checkpoint_does_not_erase_a_recorded_attention_notification() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let ready = provider.push_gated(stop_response("active", "Parent work finished."));
    let registry = registry(store.clone(), &provider);
    let handle = registry.get_or_create("attention-parent").await.unwrap();
    let parent = tokio::spawn(async move {
        drive_completion(&handle, user_turn(running_state(), "parent", "Continue.")).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while provider.requests().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let handle = registry.get_or_create("attention-side").await.unwrap();
    let mut message = notice(
        running_state(),
        true,
        300,
        pb::BackgroundTaskStatus::Unspecified,
    );
    completion(&mut message).reason =
        pb::BackgroundTaskCompletionReason::WorkerNeedsAttention as i32;
    completion(&mut message).detail = Some("Waiting for user approval".into());
    drive_completion_end(&handle, message, false).await;
    ready.notify_one();
    let (state, _) = parent.await.unwrap();
    let run = &state.subagent_runs_by_parent_tool_call_id["task-call"];
    assert_eq!(run.status, pb::SubagentRunStatus::Backgrounded as i32);
    assert_eq!(
        run.completion_reason,
        Some(pb::BackgroundTaskCompletionReason::WorkerNeedsAttention as i32)
    );
    assert_eq!(run.detail.as_deref(), Some("Waiting for user approval"));
    assert_eq!(run.completed_timestamp_ms, Some(300));
    let stored: Vec<u8> = sqlx::query_scalar(
        "SELECT state FROM cursor_subagent_runs WHERE parent_tool_call_id = 'task-call'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        pb::SubagentRunState::decode(stored.as_slice()).unwrap(),
        *run
    );
    assert_eq!(provider.requests().len(), 1);
    registry.shutdown().await;
}
