//! Native reasoning survives protocol switches without using display summaries.
use super::*;

const MODEL: &str = "gateway/DeepSeek-V4-Pro";

fn response_events(items: Vec<Value>, tool: bool) -> Vec<Value> {
    response_stream(items, tool, false)
}

fn response_stream(mut items: Vec<Value>, tool: bool, snapshot_only: bool) -> Vec<Value> {
    let mut events = vec![json!({
        "type":"response.reasoning_summary_text.delta", "delta":"DISPLAY_SUMMARY_ONLY"
    })];
    if !snapshot_only {
        for (index, item) in items.iter().enumerate() {
            events.push(
                json!({"type":"response.output_item.done", "output_index":index, "item":item}),
            );
        }
    }
    let index = items.len();
    if tool {
        let item = json!({"type":"function_call", "id":"fc_native", "call_id":CALL_ID,
            "name":"Read", "arguments":"{\"path\":\"test.txt\"}"});
        events.push(json!({"type":"response.output_item.done", "output_index":index, "item":item}));
        items.push(item);
    } else {
        events.push(
            json!({"type":"response.output_text.delta", "output_index":index, "delta":"answer"}),
        );
        items.push(json!({"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"answer"}]}));
    }
    events.push(json!({"type":"response.completed", "response":{"output":items}}));
    events
}

fn reasoning(id: &str, parts: &[&str]) -> Value {
    json!({"type":"reasoning", "id":id,
        "content":parts.iter().map(|text| json!({"type":"reasoning_text", "text":text})).collect::<Vec<_>>(),
        "summary":[{"type":"summary_text", "text":"NOT_COMPLETE_REASONING"}]
    })
}

fn append_cycle(messages: &mut Vec<CanonicalMessage>, id: &str, cycle: ModelCycleResult) {
    let calls = cycle
        .calls
        .iter()
        .map(|call| ToolCallContent {
            index: call.index,
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        })
        .collect();
    messages.push(CanonicalMessage {
        message_id: id.into(),
        role: Role::Assistant,
        origin: Origin::Assistant,
        runtime_event_id: None,
        content: MessageContent::Assistant {
            text: cycle.text,
            thinking: cycle.reasoning,
            replay_state: cycle.replay_state,
            tool_round_id: None,
            tool_calls: calls,
        },
    });
    for call in cycle.calls {
        messages.push(CanonicalMessage {
            message_id: format!("result-{}", call.call_id),
            role: Role::Tool,
            origin: Origin::Tool,
            runtime_event_id: None,
            content: MessageContent::ToolResult(ToolResultContent {
                call_id: call.call_id,
                name: call.name,
                content: "file contents".into(),
                is_error: false,
                image: None,
                provider_parts: vec![],
            }),
        });
    }
}

fn chat_finish() -> Vec<Value> {
    vec![json!({"choices":[{"index":0,"delta":{"content":"accepted"},"finish_reason":"stop"}]})]
}

#[tokio::test]
async fn responses_plain_reasoning_survives_restart_checkpoint_and_chat_continuation() {
    exercise_responses(false).await;
}

#[tokio::test]
async fn responses_final_snapshot_preserves_native_reasoning_for_chat() {
    exercise_responses(true).await;
}

async fn exercise_responses(snapshot_only: bool) {
    let mut history = vec![CanonicalMessage::text(
        "user",
        Role::User,
        Origin::User,
        "read",
    )];
    let first = Endpoint::new(response_stream(
        vec![reasoning("r0", &["earlier ", "reasoning"])],
        false,
        snapshot_only,
    ))
    .await;
    let result = cycle_model(
        first.provider(ProviderKind::OpenAiResponses).as_ref(),
        &history,
        MODEL,
    )
    .await;
    append_cycle(&mut history, "first-assistant", result);
    history.push(CanonicalMessage::text(
        "user2",
        Role::User,
        Origin::User,
        "continue",
    ));
    let source = Endpoint::new(response_stream(
        vec![
            reasoning("r1", &["read ", "the file"]),
            reasoning("r2", &[" then verify"]),
        ],
        true,
        snapshot_only,
    ))
    .await;
    let result = cycle_model(
        source.provider(ProviderKind::OpenAiResponses).as_ref(),
        &history,
        MODEL,
    )
    .await;
    assert_eq!(result.calls.len(), 1);
    assert_eq!(result.reasoning, "DISPLAY_SUMMARY_ONLY");
    append_cycle(&mut history, "tool-assistant", result);
    let mut recovered = persist_roundtrip(&history, MODEL).await;
    let before = recovered.clone();
    let mut destination = Endpoint::validating(
        chat_finish(),
        Some(vec![
            "earlier reasoning".into(),
            "read the file then verify".into(),
        ]),
    )
    .await;
    let provider = destination.provider(ProviderKind::OpenAiChat);
    assert_eq!(
        cycle_model(provider.as_ref(), &recovered, MODEL).await.text,
        "accepted"
    );
    let first = destination.requests.try_recv().unwrap();
    let messages = first["messages"].as_array().unwrap();
    let call = messages
        .iter()
        .find(|m| m.get("tool_calls").is_some())
        .unwrap();
    assert_eq!(call["tool_calls"][0]["id"], CALL_ID);
    assert_eq!(messages.last().unwrap()["tool_call_id"], CALL_ID);
    assert!(!first.to_string().contains("SUMMARY"));
    assert_eq!(
        recovered, before,
        "serialization must not rewrite persisted history"
    );
    recovered.push(CanonicalMessage::text(
        "next-user",
        Role::User,
        Origin::User,
        "explain",
    ));
    cycle_model(provider.as_ref(), &recovered, MODEL).await;
    let next = destination.requests.try_recv().unwrap();
    assert!(next["messages"].as_array().unwrap().starts_with(messages));
}

#[tokio::test]
async fn explicit_empty_chat_reasoning_survives_restart_and_replay() {
    for delta in [json!({"reasoning_content":""}), json!({"reasoning":""})] {
        let source = Endpoint::new(vec![
            json!({"choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":CALL_ID,"type":"function","function":{"name":"Read","arguments":"{\"path\":\"test.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
        ]).await;
        let result = cycle_model(
            source.provider(ProviderKind::OpenAiChat).as_ref(),
            &[],
            MODEL,
        )
        .await;
        assert_eq!(
            result
                .replay_state
                .as_ref()
                .map(|state| state.value.clone()),
            Some(json!({"reasoning_content":""}))
        );
        let mut history = Vec::new();
        append_cycle(&mut history, "assistant", result);
        if let MessageContent::Assistant { thinking, .. } = &mut history[0].content {
            *thinking = "DISPLAY_ONLY".into();
        }
        let recovered = persist_roundtrip(&history, MODEL).await;
        let destination = Endpoint::validating(chat_finish(), Some(vec![String::new()])).await;
        assert_eq!(
            cycle_model(
                destination.provider(ProviderKind::OpenAiChat).as_ref(),
                &recovered,
                MODEL
            )
            .await
            .text,
            "accepted"
        );
    }
}

#[tokio::test]
async fn unsupported_or_incomplete_reasoning_is_not_fabricated() {
    let opaque = json!({"type":"reasoning", "id":"r-secret", "encrypted_content":"OPAQUE_SECRET", "summary":[{"type":"summary_text","text":"short"}]});
    let full = reasoning("r-full", &["native reasoning"]);
    for (model, items) in [
        (MODEL, vec![opaque.clone()]),
        (MODEL, vec![full.clone(), opaque]),
        (MODEL, vec![reasoning("r-empty", &[])]),
        ("gpt-example", vec![full.clone()]),
        ("gateway/not-deepseek", vec![full]),
    ] {
        let source = Endpoint::new(response_events(items, true)).await;
        let result = cycle_model(
            source.provider(ProviderKind::OpenAiResponses).as_ref(),
            &[],
            MODEL,
        )
        .await;
        let mut history = Vec::new();
        append_cycle(&mut history, "assistant", result);
        let recovered = persist_roundtrip(&history, model).await;
        let mut destination = Endpoint::new(chat_finish()).await;
        cycle_model(
            destination.provider(ProviderKind::OpenAiChat).as_ref(),
            &recovered,
            model,
        )
        .await;
        let request = destination.requests.try_recv().unwrap();
        let messages = request["messages"].as_array().unwrap();
        assert_eq!(messages.last().unwrap()["role"], "tool");
        assert!(messages
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap()
            .get("reasoning_content")
            .is_none());
        assert!(!request.to_string().contains("OPAQUE_SECRET"));
        assert!(!request.to_string().contains("SUMMARY"));
    }
}
