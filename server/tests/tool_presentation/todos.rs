//! Todo cards must show the previous list and the full committed result.
use super::*;
use serde_json::{json, Value};

fn todo_call(id: &str, arguments: &Value) -> Vec<ModelEvent> {
    vec![
        ModelEvent::Start {
            model_call_id: format!("model-{id}"),
        },
        ModelEvent::ToolCallStart {
            index: 0,
            call_id: id.into(),
            name: "TodoWrite".into(),
        },
        ModelEvent::ToolCallArgumentsDelta {
            index: 0,
            delta: arguments.to_string(),
        },
        ModelEvent::ToolCallEnd { index: 0 },
        ModelEvent::Done(FinishReason::ToolUse),
    ]
}

fn finished() -> Vec<ModelEvent> {
    vec![
        ModelEvent::Start {
            model_call_id: "final".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("done".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]
}

fn registry(
    store: cursor_server::store::Store,
    provider: &fake_provider::FakeProvider,
) -> TransportRegistry {
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    TransportRegistry::new(
        store,
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    )
}

#[tokio::test]
async fn todo_completion_keeps_full_state_across_checkpoint_resume_and_clear() {
    exercise_todos(true).await;
}

#[tokio::test]
async fn todo_completion_uses_server_history_when_cursor_checkpoint_is_omitted() {
    exercise_todos(false).await;
}

async fn exercise_todos(with_checkpoint: bool) {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let create = json!({"merge": false, "todos": [
        {"id": "read", "content": "Read fixture", "status": "completed"},
        {"id": "verify", "content": "Verify fixture", "status": "in_progress", "dependencies": ["read"]},
    ]});
    let patch = json!({"merge": true, "todos": [{"id": "verify", "status": "completed"}]});
    let clear = json!({"merge": false, "todos": []});
    provider.push(todo_call("create", &create));
    provider.push(finished());
    provider.push(todo_call("patch", &patch));
    provider.push(todo_call("clear", &clear));
    provider.push(finished());
    let (created, checkpoint) =
        run_todos(&registry(store.clone(), &provider), "create-run", None).await;
    assert_eq!(checkpoint.todos.len(), 2);
    let (updated, final_checkpoint) = run_todos(
        &registry(store.clone(), &provider),
        "update-run",
        with_checkpoint.then_some(checkpoint),
    )
    .await;
    let merged = success(&updated[0]);
    assert_eq!(
        merged.todos.len(),
        2,
        "status-only merge must retain unchanged todos"
    );
    assert_eq!(merged.total_count, 2);
    assert_eq!(merged.todos[1].content, "Verify fixture");
    assert_eq!(merged.todos[1].dependencies, ["read"]);
    assert_eq!(merged.todos[1].status, pb::TodoStatus::Completed as i32);
    let previous = &updated[0].args.as_ref().unwrap().todos;
    assert_eq!(previous.len(), 2);
    assert_eq!(previous[1].status, pb::TodoStatus::InProgress as i32);
    assert!(
        created[0].args.as_ref().unwrap().todos.is_empty(),
        "new todos must display as additions"
    );
    assert_eq!(success(&created[0]).todos.len(), 2);
    assert_eq!(updated[1].args.as_ref().unwrap().todos.len(), 2);
    assert!(updated[1]
        .args
        .as_ref()
        .unwrap()
        .todos
        .iter()
        .all(|todo| todo.status == pb::TodoStatus::Completed as i32));
    assert!(success(&updated[1]).todos.is_empty());
    assert_eq!(success(&updated[1]).total_count, 0);
    assert!(final_checkpoint.todos.is_empty());

    // Changing the native card must not rewrite the canonical tool contract.
    let messages = store
        .load_current_messages(&ConversationId::new("todos-conversation"))
        .await
        .unwrap();
    for (id, expected) in [("create", &create), ("patch", &patch), ("clear", &clear)] {
        let result = messages
            .iter()
            .find_map(|message| match &message.content {
                MessageContent::ToolResult(result) if result.call_id == id => Some(result),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            &serde_json::from_str::<Value>(&result.content).unwrap(),
            expected
        );
        let call = messages
            .iter()
            .find_map(|message| match &message.content {
                MessageContent::Assistant { tool_calls, .. } => {
                    tool_calls.iter().find(|call| call.call_id == id)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(&call.arguments, expected);
    }
    assert_eq!(provider.requests().len(), 5);
}

fn success(tool: &pb::UpdateTodosToolCall) -> &pb::UpdateTodosSuccess {
    let Some(pb::update_todos_result::Result::Success(success)) = tool
        .result
        .as_ref()
        .and_then(|result| result.result.as_ref())
    else {
        panic!("TodoWrite success")
    };
    success
}

async fn run_todos(
    registry: &TransportRegistry,
    id: &str,
    checkpoint: Option<pb::ConversationStateStructure>,
) -> (Vec<pb::UpdateTodosToolCall>, pb::ConversationStateStructure) {
    let mut request = client_run();
    let Some(pb::agent_client_message::Message::RunRequest(run)) = &mut request.message else {
        unreachable!()
    };
    run.conversation_id = Some("todos-conversation".into());
    run.run_id = Some(id.into());
    run.conversation_state = checkpoint;
    let Some(pb::conversation_action::Action::UserMessageAction(action)) =
        run.action.as_mut().unwrap().action.as_mut()
    else {
        unreachable!()
    };
    action.user_message.as_mut().unwrap().message_id = format!("user-{id}");
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
            return (completed, checkpoint.unwrap());
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
                    if let Some(pb::tool_call::Tool::UpdateTodosToolCall(tool)) =
                        call.tool_call.unwrap().tool
                    {
                        completed.push(tool);
                    }
                }
            }
            _ => {}
        }
    }
}
