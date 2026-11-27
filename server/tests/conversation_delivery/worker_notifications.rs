//! Current Cursor notification wire values, including fields missing in older schemas.
use super::*;

#[derive(Clone, PartialEq, Message)]
struct NotificationMetadata {
    #[prost(uint64, optional, tag = "12")]
    completed_at_ms: Option<u64>,
    #[prost(bool, tag = "13")]
    record_only: bool,
}

fn notification(
    reason: i32,
    timestamp: Option<u64>,
    record_only: bool,
    detail: &str,
    state: pb::ConversationStateStructure,
) -> pb::AgentClientMessage {
    let mut request = completion_run_with_detail("worker", "notification", state, detail);
    let Some(pb::agent_client_message::Message::RunRequest(run)) = request.message.as_mut() else {
        unreachable!()
    };
    let Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(action)) =
        run.action.as_mut().unwrap().action.as_mut()
    else {
        unreachable!()
    };
    let completion = &mut action.completions[0];
    completion.reason = reason;
    if reason != 1 {
        completion.status = pb::BackgroundTaskStatus::Unspecified as i32;
    }
    // Encode the actual new fields so this regression also detects a schema
    // silently discarding them, rather than depending on generated Rust fields.
    let mut wire = completion.encode_to_vec();
    NotificationMetadata {
        completed_at_ms: timestamp,
        record_only,
    }
    .encode(&mut wire)
    .unwrap();
    *completion = pb::BackgroundTaskCompletion::decode(wire.as_slice()).unwrap();
    request
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

fn completions(message: &mut pb::AgentClientMessage) -> &mut Vec<pb::BackgroundTaskCompletion> {
    let Some(pb::agent_client_message::Message::RunRequest(run)) = message.message.as_mut() else {
        unreachable!()
    };
    let Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(action)) =
        run.action.as_mut().unwrap().action.as_mut()
    else {
        unreachable!()
    };
    &mut action.completions
}

#[tokio::test]
async fn worker_notifications_are_distinct_from_task_completion() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(store, &provider);
    let mut state = pb::ConversationStateStructure::default();
    for (reason, label, instruction) in [
        (3, "worker_reparented", "attached"),
        (4, "worker_message", "may still be running"),
        (
            5,
            "worker_needs_attention",
            "Don't answer or approve for the user",
        ),
        (6, "project_session_message", "untrusted"),
    ] {
        provider.push(stop_response(&format!("reply-{reason}"), "Recorded."));
        let handle = registry
            .get_or_create(&format!("notice-{reason}"))
            .await
            .unwrap();
        let mut notice = notification(reason, Some(100), false, "Update from worker.", state);
        if reason == 6 {
            // A message from another project chat need not identify a child.
            completions(&mut notice)[0].subagent_id = None;
            completions(&mut notice)[0].tool_call_id = None;
        }
        (state, _) = drive_completion(&handle, notice).await;
        let requests = provider.requests();
        let request = requests.last().unwrap();
        let text = serde_json::to_string(&request.history.last().unwrap().content).unwrap();
        assert!(text.contains(&format!("reason: {label}")), "{text}");
        assert!(text.contains(instruction), "{text}");
        assert!(!text.contains("The following task has finished"));
        assert!(!text.contains("status: success"));
        if requests.len() > 1 {
            let previous = &requests[requests.len() - 2];
            assert!(request.history.starts_with(&previous.history));
            assert_eq!(request.prompt, previous.prompt);
        }
    }
    provider.push(stop_response("finished", "Done."));
    let handle = registry.get_or_create("notice-finished").await.unwrap();
    drive_completion(
        &handle,
        notification(1, Some(100), false, "Update from worker.", state),
    )
    .await;
    assert_eq!(
        provider.requests().len(),
        5,
        "messages must not consume the eventual completion"
    );
    registry.shutdown().await;
}

#[tokio::test]
async fn repeated_worker_messages_survive_restart_without_replaying_consumed_notifications() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let first = registry(store.clone(), &provider);
    provider.push(stop_response("reply-1", "Recorded."));
    let handle = first.get_or_create("message-1").await.unwrap();
    let (state, _) = drive_completion(
        &handle,
        notification(4, Some(100), false, "Ready.", Default::default()),
    )
    .await;
    first.shutdown().await;
    let restarted = registry(store.clone(), &provider);
    let handle = restarted.get_or_create("message-retry").await.unwrap();
    drive_completion_end(
        &handle,
        notification(4, Some(100), false, "Ready.", state.clone()),
        false,
    )
    .await;
    assert_eq!(provider.requests().len(), 1);

    provider.push(stop_response("reply-2", "Recorded again."));
    let handle = restarted.get_or_create("message-2").await.unwrap();
    let (state, _) =
        drive_completion(&handle, notification(4, Some(200), false, "Ready.", state)).await;
    provider.push(stop_response("reply-3", "Another update."));
    let handle = restarted.get_or_create("message-3").await.unwrap();
    let (state, _) = drive_completion(
        &handle,
        notification(4, Some(200), false, "Different update.", state),
    )
    .await;
    let handle = restarted.get_or_create("old-message-retry").await.unwrap();
    drive_completion_end(
        &handle,
        notification(4, Some(100), false, "Ready.", state),
        false,
    )
    .await;
    let requests = provider.requests();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].history.starts_with(&requests[0].history));
    assert!(requests[2].history.starts_with(&requests[1].history));
    let runtime_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE message_id LIKE 'runtime:background-completed:%'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(runtime_count, 3);
    restarted.shutdown().await;
}

#[tokio::test]
async fn record_only_notifications_do_not_wake_or_consume_a_later_delivery() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    // A response makes an incorrect wakeup observable without hanging the test.
    provider.push(stop_response("unexpected", "Unexpected."));
    let registry = registry(store.clone(), &provider);
    let handle = registry.get_or_create("record-only").await.unwrap();
    let messages = drive_completion_end(
        &handle,
        notification(1, Some(100), true, "Done.", Default::default()),
        false,
    )
    .await;
    assert_eq!(
        messages.len(),
        1,
        "record-only input should only acknowledge the turn"
    );
    assert!(provider.requests().is_empty());
    let handle = registry.get_or_create("record-delivery").await.unwrap();
    drive_completion(
        &handle,
        notification(1, Some(100), false, "Done.", Default::default()),
    )
    .await;
    assert_eq!(provider.requests().len(), 1);
    registry.shutdown().await;
}

#[tokio::test]
async fn mixed_rebatched_notifications_only_deliver_new_visible_events() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(stop_response("batch-1", "Recorded."));
    provider.push(stop_response("batch-2", "Done."));
    let registry = registry(store, &provider);
    let mut batch = notification(4, Some(100), false, "Visible update.", Default::default());
    let mut hidden = notification(4, Some(101), true, "Hidden update.", Default::default());
    let mut attention = notification(
        5,
        Some(102),
        false,
        "User input needed.",
        Default::default(),
    );
    completions(&mut batch).append(completions(&mut hidden));
    completions(&mut batch).append(completions(&mut attention));
    let handle = registry.get_or_create("mixed-batch").await.unwrap();
    let (state, _) = drive_completion(&handle, batch.clone()).await;
    let text = serde_json::to_string(&provider.requests()[0].history).unwrap();
    assert!(text.contains("Visible update."));
    assert!(text.contains("User input needed."));
    assert!(!text.contains("Hidden update."));

    let mut next = notification(1, Some(103), false, "Finished.", state);
    completions(&mut batch).reverse();
    completions(&mut next).append(completions(&mut batch));
    let handle = registry.get_or_create("mixed-batch-retry").await.unwrap();
    drive_completion(&handle, next).await;
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].history.starts_with(&requests[0].history));
    let text = serde_json::to_string(&requests[1].history).unwrap();
    assert_eq!(text.matches("detail: Visible update.").count(), 1);
    assert_eq!(text.matches("detail: User input needed.").count(), 1);
    assert_eq!(text.matches("detail: Finished.").count(), 1);
    assert!(!text.contains("Hidden update."));
    registry.shutdown().await;
}

#[tokio::test]
async fn worker_messages_keep_the_clients_batch_order() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(stop_response("ordered", "Recorded in order."));
    let registry = registry(store, &provider);
    let mut batch = notification(4, Some(100), false, "First update.", Default::default());
    let mut second = notification(4, Some(100), false, "Second update.", Default::default());
    completions(&mut batch).append(completions(&mut second));
    let handle = registry.get_or_create("ordered-batch").await.unwrap();
    drive_completion(&handle, batch).await;
    let text = serde_json::to_string(&provider.requests()[0].history).unwrap();
    assert!(
        text.find("First update.").unwrap() < text.find("Second update.").unwrap(),
        "{text}"
    );
    registry.shutdown().await;
}

#[tokio::test]
async fn user_driven_worker_updates_preserve_user_ownership() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(store, &provider);
    let mut state = pb::ConversationStateStructure::default();
    for reason in [3, 4, 5, 1] {
        provider.push(stop_response(&format!("user-driven-{reason}"), "Recorded."));
        let mut notice = notification(reason, Some(100), false, "User's worker update.", state);
        completions(&mut notice)[0].notification_context =
            pb::BackgroundTaskNotificationContext::UserDrivenInteractiveChild as i32;
        let handle = registry
            .get_or_create(&format!("user-driven-{reason}"))
            .await
            .unwrap();
        (state, _) = drive_completion(&handle, notice).await;
        let requests = provider.requests();
        let text = serde_json::to_string(&requests.last().unwrap().history.last().unwrap().content)
            .unwrap();
        assert!(text.contains("started_by: user"));
        assert!(text.contains("don't message the user or take over the worker's thread"));
        assert!(!text.contains(FOLLOW_UP));
    }
    registry.shutdown().await;
}

#[tokio::test]
async fn worker_message_joins_the_current_run_without_interrupting_it() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let ready = provider.push_gated(stop_response("active", "Original response."));
    provider.push(stop_response("follow-up", "Processed the message."));
    let registry = registry(store.clone(), &provider);
    let handle = registry.get_or_create("active-parent").await.unwrap();
    let parent = tokio::spawn(async move {
        drive_completion(
            &handle,
            completion_run("first", "active", Default::default()),
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
    let handle = registry.get_or_create("incoming-message").await.unwrap();
    let incoming = tokio::spawn(async move {
        drive_forwarded_completion(
            &handle,
            notification(4, Some(100), false, "Incoming update.", Default::default()),
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    ready.notify_one();
    incoming.await.unwrap();
    parent.await.unwrap();
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].history.starts_with(&requests[0].history));
    let text = serde_json::to_string(&requests[1].history).unwrap();
    assert!(text.contains("Original response."));
    assert!(text.contains("Incoming update."));
    let statuses: Vec<String> =
        sqlx::query_scalar("SELECT status FROM runs WHERE conversation_id = 'parent-conversation'")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(statuses, ["completed"]);
    registry.shutdown().await;
}
