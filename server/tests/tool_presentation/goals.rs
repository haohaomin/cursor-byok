//! Exercises goals through the real Run transport with a deterministic provider.
use super::todos::{finished, registry, todo_call};
use super::*;
use serde_json::json;
const CONVERSATION: &str = "f5e623a8-58ba-48ea-a6af-177c12d88327";
fn goal_call(id: &str, name: &str, arguments: serde_json::Value) -> Vec<ModelEvent> {
    let mut events = todo_call(id, &arguments);
    if let ModelEvent::ToolCallStart {
        name: tool_name, ..
    } = &mut events[1]
    {
        *tool_name = name.into();
    }
    events
}
fn active(state: &pb::ConversationStateStructure) -> &pb::GoalState {
    let goal = state
        .goal_state
        .as_ref()
        .expect("CreateGoal must persist a goal");
    assert_eq!(goal.status, pb::GoalStatus::Active as i32);
    goal
}
#[tokio::test]
async fn goal_creates_continues_completes_and_rejects_stale_wakeups() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Finish fixture verification"}),
    ));
    provider.push(finished());
    provider.push(goal_call(
        "finish",
        "UpdateGoal",
        json!({"status":"complete"}),
    ));
    provider.push(finished());
    let (cards, created) = run_goal(
        &registry(store.clone(), &provider),
        "create",
        None,
        Some("Create a goal to finish fixture verification"),
    )
    .await;
    let initial = active(&created).clone();
    assert_eq!(initial.objective, "Finish fixture verification");
    assert_eq!(initial.agent_session_id.as_deref(), Some("goal-session"));
    assert!(uuid::Uuid::parse_str(&initial.goal_id).is_ok());
    assert!(matches!(
        cards[0].tool,
        Some(pb::tool_call::Tool::CreateGoalToolCall(_))
    ));
    let (cards, completed) = run_goal(
        &registry(store.clone(), &provider),
        "continue",
        Some(created.clone()),
        None,
    )
    .await;
    let goal = completed.goal_state.as_ref().unwrap();
    assert_eq!(goal.status, pb::GoalStatus::Complete as i32);
    assert_eq!(goal.continuation_count, 1);
    assert_eq!(goal.idle_continuations_without_tool_calls, 0);
    assert!(goal.last_accrued_at_ms.is_none());
    assert!(matches!(
        cards[0].tool,
        Some(pb::tool_call::Tool::UpdateGoalToolCall(_))
    ));
    for (id, state) in [("done-retry", completed.clone()), ("stale-retry", created)] {
        let (_, ignored) =
            run_goal(&registry(store.clone(), &provider), id, Some(state), None).await;
        assert_eq!(ignored.goal_state, completed.goal_state);
    }
    let requests = provider.requests();
    assert_eq!(requests.len(), 4);
    for (before, after) in requests[1].history.iter().zip(&requests[2].history) {
        // Checkpoint hydration assigns internal IDs to assistant/tool roots;
        // provider serializers consume role/content, not those internal IDs.
        assert_eq!(before.role, after.role);
        assert_eq!(
            before.content, after.content,
            "provider history prefix changed"
        );
    }
    assert!(requests[2].history.len() >= requests[1].history.len());
    assert_eq!(requests[2].prompt, requests[1].prompt);
    let history = format!("{:?}", requests[2].history);
    assert!(history.contains("Finish fixture verification"));
    for name in ["CreateGoal", "UpdateGoal"] {
        assert!(requests[0]
            .prompt
            .tools
            .iter()
            .any(|tool| tool.name == name));
    }
}
#[tokio::test]
async fn goal_pauses_after_three_continuations_without_tools() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Verify work"}),
    ));
    for _ in 0..4 {
        provider.push(finished());
    }
    let (_, mut checkpoint) = run_goal(
        &registry(store.clone(), &provider),
        "create",
        None,
        Some("Create goal"),
    )
    .await;
    active(&checkpoint);
    for i in 1..=4 {
        let (_, next) = run_goal(
            &registry(store.clone(), &provider),
            &format!("idle-{i}"),
            Some(checkpoint),
            None,
        )
        .await;
        let goal = next.goal_state.as_ref().unwrap();
        assert_eq!(goal.continuation_count, i.min(3));
        assert_eq!(goal.idle_continuations_without_tool_calls, i.min(3));
        assert_eq!(
            goal.status,
            if i == 4 {
                pb::GoalStatus::Paused
            } else {
                pb::GoalStatus::Active
            } as i32
        );
        checkpoint = next;
    }
    assert_eq!(provider.requests().len(), 5);
}
#[tokio::test]
async fn goal_survives_compaction_and_missing_client_checkpoint() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Keep this precise objective"}),
    ));
    provider.push(finished());
    provider.push(finished()); // Manual summary intentionally omits the goal.
    provider.push(finished()); // Continued work after compaction.
    provider.push(goal_call(
        "complete",
        "UpdateGoal",
        json!({"status":"complete"}),
    ));
    provider.push(finished());
    let (_, created) = run_goal(
        &registry(store.clone(), &provider),
        "create",
        None,
        Some("Create goal"),
    )
    .await;
    let id = active(&created).goal_id.clone();
    let (_, summary) = run_goal(
        &registry(store.clone(), &provider),
        "summary",
        Some(created),
        Some("/summarize"),
    )
    .await;
    assert_eq!(summary.self_summary_count, 1);
    assert_eq!(active(&summary).goal_id, id);
    let messages = store
        .load_current_messages(&ConversationId::new(CONVERSATION))
        .await
        .unwrap();
    assert!(messages
        .iter()
        .all(|message| !matches!(message.content, MessageContent::ToolResult(_))));
    let (_, continued) = run_goal(&registry(store.clone(), &provider), "resume", None, None).await;
    assert_eq!(active(&continued).objective, "Keep this precise objective");
    assert_eq!(active(&continued).continuation_count, 1);
    let (_, completed) =
        run_goal(&registry(store.clone(), &provider), "complete", None, None).await;
    assert_eq!(
        completed.goal_state.as_ref().unwrap().status,
        pb::GoalStatus::Complete as i32
    );
    assert_eq!(completed.goal_state.as_ref().unwrap().goal_id, id);
    assert_eq!(provider.requests().len(), 6);
}

#[tokio::test]
async fn goal_honors_client_pause_clear_and_ownership() {
    for status in [pb::GoalStatus::Paused, pb::GoalStatus::Cleared] {
        let (_dir, store) = fixtures::temp_store().await;
        let provider = fake_provider::FakeProvider::default();
        provider.push(goal_call(
            "create",
            "CreateGoal",
            json!({"objective":"Verify work"}),
        ));
        provider.push(finished());
        provider.push(finished()); // Explicit user message after a pause.
        let (_, original) = run_goal(
            &registry(store.clone(), &provider),
            "create",
            None,
            Some("Create goal"),
        )
        .await;
        let mut paused = original.clone();
        paused.goal_state.as_mut().unwrap().status = status as i32;
        let (_, paused) = run_goal(
            &registry(store.clone(), &provider),
            "pause",
            Some(paused),
            None,
        )
        .await;
        assert_eq!(paused.goal_state.as_ref().unwrap().status, status as i32);
        let (_, stale) = run_goal(
            &registry(store.clone(), &provider),
            "old-active",
            Some(original),
            None,
        )
        .await;
        assert_eq!(stale.goal_state.as_ref().unwrap().status, status as i32);
        assert_eq!(provider.requests().len(), 2);
        let (_, resumed) = run_goal(
            &registry(store.clone(), &provider),
            "user",
            None,
            Some("Continue working"),
        )
        .await;
        assert_eq!(
            resumed.goal_state.as_ref().unwrap().status,
            if status == pb::GoalStatus::Paused {
                pb::GoalStatus::Active
            } else {
                pb::GoalStatus::Cleared
            } as i32
        );
        let mut foreign = resumed;
        foreign.goal_state.as_mut().unwrap().agent_session_id = Some("another-session".into());
        foreign.goal_state.as_mut().unwrap().status = pb::GoalStatus::Active as i32;
        let _ = run_goal(
            &registry(store.clone(), &provider),
            "foreign",
            Some(foreign),
            None,
        )
        .await;
        assert_eq!(provider.requests().len(), 3);
    }
}

#[tokio::test]
async fn failed_goal_update_keeps_existing_state_and_tools_reset_idle_count() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "missing",
        "UpdateGoal",
        json!({"status":"complete"}),
    ));
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Verify work"}),
    ));
    provider.push(finished());
    provider.push(finished());
    provider.push(todo_call("work", &json!({"merge":false,"todos":[]})));
    provider.push(finished());
    let (cards, created) = run_goal(
        &registry(store.clone(), &provider),
        "create",
        None,
        Some("Create goal"),
    )
    .await;
    let Some(pb::tool_call::Tool::UpdateGoalToolCall(tool)) = &cards[0].tool else {
        panic!("native UpdateGoal card")
    };
    assert!(matches!(
        tool.result.as_ref().unwrap().result,
        Some(pb::update_goal_result::Result::Error(_))
    ));
    let (_, idle) = run_goal(
        &registry(store.clone(), &provider),
        "idle",
        Some(created),
        None,
    )
    .await;
    assert_eq!(active(&idle).idle_continuations_without_tool_calls, 1);
    let (_, worked) = run_goal(
        &registry(store.clone(), &provider),
        "work",
        Some(idle),
        None,
    )
    .await;
    assert_eq!(active(&worked).idle_continuations_without_tool_calls, 0);
    assert_eq!(active(&worked).continuation_count, 2);
}

#[tokio::test]
async fn goal_retains_state_when_automatic_compaction_removes_tool_history() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Preserve goal after automatic summary"}),
    ));
    provider.push(finished());
    provider.push_error(cursor_server::Error::Provider(
        "context_length_exceeded".into(),
    ));
    provider.push(finished()); // Summary has no machine-readable goal state.
    provider.push(goal_call(
        "finish",
        "UpdateGoal",
        json!({"status":"complete"}),
    ));
    provider.push(finished());
    let (_, created) = run_goal(
        &registry(store.clone(), &provider),
        "create",
        None,
        Some("Create goal"),
    )
    .await;
    let id = active(&created).goal_id.clone();
    let (_, completed) = run_goal(
        &registry(store.clone(), &provider),
        "overflow",
        Some(created),
        None,
    )
    .await;
    assert_eq!(completed.self_summary_count, 1);
    let goal = completed.goal_state.as_ref().unwrap();
    assert_eq!(goal.status, pb::GoalStatus::Complete as i32);
    assert_eq!(goal.goal_id, id);
    assert_eq!(goal.continuation_count, 1);
    assert_eq!(provider.requests().len(), 6);
}

#[tokio::test]
async fn pending_tools_prevent_goal_wakeups() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Wait for the tool"}),
    ));
    provider.push(finished());
    let (_, mut pending) = run_goal(
        &registry(store.clone(), &provider),
        "create",
        None,
        Some("Create goal"),
    )
    .await;
    pending.pending_tool_calls = vec![json!({"role":"assistant","content":[{"type":"tool-call","toolName":"Shell","toolCallId":"pending","args":{}}]}).to_string()];
    let (_, unchanged) = run_goal(
        &registry(store.clone(), &provider),
        "premature",
        Some(pending),
        None,
    )
    .await;
    assert_eq!(active(&unchanged).continuation_count, 0);
    assert_eq!(provider.requests().len(), 2);
}

#[tokio::test]
async fn goal_id_distinguishes_reused_tool_ids_in_different_rounds() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"First objective"}),
    ));
    provider.push(goal_call(
        "finish",
        "UpdateGoal",
        json!({"status":"complete"}),
    ));
    provider.push(finished());
    let mut second = goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Second objective"}),
    );
    second[0] = ModelEvent::Start {
        model_call_id: "second-model".into(),
    };
    provider.push(second);
    provider.push(finished());
    let (_, first) = run_goal(
        &registry(store.clone(), &provider),
        "first",
        None,
        Some("Create first goal"),
    )
    .await;
    let (_, second) = run_goal(
        &registry(store.clone(), &provider),
        "second",
        Some(first.clone()),
        Some("Create another goal"),
    )
    .await;
    assert_eq!(active(&second).objective, "Second objective");
    assert_ne!(active(&second).goal_id, first.goal_state.unwrap().goal_id);
}

#[tokio::test]
async fn simultaneous_goal_wakeups_start_only_one_model_run() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Continue exactly once"}),
    ));
    provider.push(finished());
    provider.push(finished());
    let registry = registry(store.clone(), &provider);
    let (_, created) = run_goal(&registry, "create", None, Some("Create goal")).await;
    let (first, second) = tokio::join!(
        run_goal(&registry, "wake-a", Some(created.clone()), None),
        run_goal(&registry, "wake-b", Some(created), None),
    );
    assert_eq!(provider.requests().len(), 3);
    assert_eq!(
        first
            .1
            .goal_state
            .unwrap()
            .continuation_count
            .max(second.1.goal_state.unwrap().continuation_count),
        1
    );
}

#[tokio::test]
async fn goal_wakeup_does_not_interrupt_an_active_user_run() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Keep working"}),
    ));
    provider.push(finished());
    let release = provider.push_gated(finished());
    let registry = registry(store.clone(), &provider);
    let (_, created) = run_goal(&registry, "create", None, Some("Create goal")).await;
    let (user, _) = tokio::join!(
        run_goal(
            &registry,
            "user-priority",
            Some(created.clone()),
            Some("Do this next")
        ),
        async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while provider.requests().len() < 3 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let ignored = run_goal(&registry, "late-goal", Some(created), None).await;
            release.notify_one();
            ignored
        },
    );
    assert_eq!(active(&user.1).continuation_count, 0);
    assert_eq!(provider.requests().len(), 3);
}

#[tokio::test]
async fn cursor_toolbar_can_resume_a_paused_goal_and_remove_it() {
    let (_dir, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    provider.push(goal_call(
        "create",
        "CreateGoal",
        json!({"objective":"Resume from toolbar"}),
    ));
    provider.push(finished());
    provider.push(finished());
    let registry = registry(store.clone(), &provider);
    let (_, mut paused) = run_goal(&registry, "create", None, Some("Create goal")).await;
    paused.goal_state.as_mut().unwrap().status = pb::GoalStatus::Paused as i32;
    let (_, paused) = run_goal(&registry, "pause", Some(paused), None).await;
    let mut resumed = paused.clone();
    let goal = resumed.goal_state.as_mut().unwrap();
    goal.status = pb::GoalStatus::Active as i32;
    goal.idle_continuations_without_tool_calls = 0;
    goal.last_accrued_at_ms = Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
    );
    let (_, resumed) = run_goal(&registry, "toolbar-resume", Some(resumed), None).await;
    assert_eq!(active(&resumed).continuation_count, 1);
    let mut cleared = resumed.clone();
    cleared.goal_state = None; // Cursor clearGoal removes the optional field.
    let (_, cleared) = run_goal(&registry, "toolbar-clear", Some(cleared), None).await;
    assert!(cleared.goal_state.is_none());
    let (_, stale) = run_goal(&registry, "old-active", Some(resumed), None).await;
    assert!(stale.goal_state.is_none());
    let (_, recovered) = run_goal(&registry, "missing-checkpoint", None, None).await;
    assert!(recovered.goal_state.is_none());
    assert_eq!(provider.requests().len(), 3);
}

#[tokio::test]
async fn delayed_goal_wakeup_keeps_later_user_work_and_compaction() {
    for compact in [false, true] {
        let (_dir, store) = fixtures::temp_store().await;
        let provider = fake_provider::FakeProvider::default();
        provider.push(goal_call(
            "create",
            "CreateGoal",
            json!({"objective":"Keep newer work"}),
        ));
        provider.push(finished());
        provider.push(finished());
        provider.push(finished());
        let registry = registry(store.clone(), &provider);
        let (_, old) = run_goal(&registry, "create", None, Some("Create goal")).await;
        let (_, newer) = run_goal(
            &registry,
            "user-work",
            Some(old.clone()),
            Some(if compact {
                "/summarize"
            } else {
                "NEW_USER_WORK_MUST_REMAIN"
            }),
        )
        .await;
        let (_, continued) = run_goal(&registry, "delayed", Some(old), None).await;
        assert_eq!(active(&continued).continuation_count, 1);
        assert_eq!(continued.self_summary_count, newer.self_summary_count);
        let requests = provider.requests();
        assert_eq!(requests.len(), 4);
        if compact {
            assert_eq!(continued.self_summary_count, 1);
            assert!(!format!("{:?}", requests[3].history).contains("CreateGoal"));
        } else {
            assert!(requests[3].history.len() > requests[2].history.len());
            for (old, new) in requests[2].history.iter().zip(&requests[3].history) {
                assert_eq!(old.role, new.role);
                assert_eq!(old.content, new.content);
            }
            assert!(format!("{:?}", requests[3].history).contains("NEW_USER_WORK_MUST_REMAIN"));
        }
    }
}

async fn run_goal(
    registry: &TransportRegistry,
    id: &str,
    checkpoint: Option<pb::ConversationStateStructure>,
    text: Option<&str>,
) -> (Vec<pb::ToolCall>, pb::ConversationStateStructure) {
    let previous_checkpoint = checkpoint.clone();
    let mut request = client_run();
    let Some(pb::agent_client_message::Message::RunRequest(run)) = &mut request.message else {
        unreachable!()
    };
    run.conversation_id = Some(CONVERSATION.into());
    run.run_id = Some(id.into());
    run.conversation_state = checkpoint;
    run.agent_session_id = Some("goal-session".into());
    if let Some(text) = text {
        let Some(pb::conversation_action::Action::UserMessageAction(action)) =
            run.action.as_mut().unwrap().action.as_mut()
        else {
            unreachable!()
        };
        action.user_message.as_mut().unwrap().message_id = format!("user-{id}");
        action.user_message.as_mut().unwrap().text = text.into();
    } else {
        run.action = Some(pb::ConversationAction {
            action: Some(pb::conversation_action::Action::GoalContinuationAction(
                pb::GoalContinuationAction {},
            )),
            ..Default::default()
        });
    }
    let handle = registry.get_or_create(id).await.unwrap();
    let mut output = handle.subscribe();
    handle
        .command(TransportCommand::Append {
            seqno: 0,
            message: Box::new(request),
        })
        .await
        .unwrap();
    let mut seqno = 1;
    let mut completed = Vec::new();
    let mut checkpoint = None;
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), output.recv())
            .await
            .unwrap()
            .unwrap();
        let (flags, payload) = connect::decode_frames(&frame).unwrap().pop().unwrap();
        if flags & connect::END_STREAM_FLAG != 0 {
            assert_eq!(payload.as_ref(), b"{}");
            return (
                completed,
                checkpoint.or(previous_checkpoint).unwrap_or_default(),
            );
        }
        let message = pb::AgentServerMessage::decode(payload).unwrap();
        match message.message {
            Some(pb::agent_server_message::Message::ConversationCheckpointUpdate(state)) => {
                checkpoint = Some(state)
            }
            Some(pb::agent_server_message::Message::KvServerMessage(kv)) => {
                handle
                    .command(TransportCommand::Append {
                        seqno,
                        message: Box::new(pb::AgentClientMessage {
                            message: Some(pb::agent_client_message::Message::KvClientMessage(
                                pb::KvClientMessage {
                                    id: kv.id,
                                    message: Some(pb::kv_client_message::Message::SetBlobResult(
                                        pb::SetBlobResult { error: None },
                                    )),
                                },
                            )),
                        }),
                    })
                    .await
                    .unwrap();
                seqno += 1;
            }
            Some(pb::agent_server_message::Message::InteractionUpdate(update)) => {
                if let Some(pb::interaction_update::Message::ToolCallCompleted(call)) =
                    update.message
                {
                    completed.push(call.tool_call.unwrap());
                }
            }
            _ => {}
        }
    }
}
