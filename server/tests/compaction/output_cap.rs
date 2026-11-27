//! Output budgets and bounded retries must preserve the complete source history.
use super::*;

#[tokio::test]
async fn summary_respects_the_selected_models_output_limit() {
    let (_directory, store) = fixtures::temp_store().await;
    let mut input = windowed_model("output-limit", Some(100_000));
    input.max_completion_tokens = Some(1024);
    let model = store.create_model(&input).await.unwrap();
    let provider = fake_provider::FakeProvider::default();
    provider.push(text_response("COMPLETED_TASK", 4000, 20));
    provider.push_error(cursor_server::Error::Provider(
        "context_length_exceeded".into(),
    ));
    provider.push(text_response("DURABLE_SUMMARY", 1000, 20));
    provider.push(text_response("continued", 500, 10));
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    let registry = TransportRegistry::new(
        store,
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    );
    let first = run(
        &registry,
        "cap-first",
        user_request("cap", "u1", "ORIGINAL_GOAL", &model.model_hash, None),
    )
    .await;
    let second = run(
        &registry,
        "cap-second",
        user_request(
            "cap",
            "u2",
            "continue",
            &model.model_hash,
            first.checkpoints.last().cloned(),
        ),
    )
    .await;
    assert!(
        matches!(second.end_stream, EndStream::Success {}),
        "{:?}",
        second.end_stream
    );
    assert_eq!(second.summary_completed, 1);
    let requests = provider.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[2].model.max_output_tokens, Some(1024));
    assert!(serde_json::to_string(&requests[2].history)
        .unwrap()
        .contains("COMPLETED_TASK"));
}

#[tokio::test]
async fn truncated_summary_retries_smaller_input_with_unknown_or_small_window() {
    for window in [None, Some(32_768)] {
        let (_directory, store) = fixtures::temp_store().await;
        store
            .set_compaction_settings(cursor_server::store::CompactionSettingsUpdate {
                auto_compaction_output_tokens: Some(65_536),
                ..Default::default()
            })
            .await
            .unwrap();
        let model = store
            .create_model(&windowed_model("retry-cap", window))
            .await
            .unwrap();
        let provider = fake_provider::FakeProvider::default();
        let original = format!("COMPLETED_TASK {}", "0123456789".repeat(3_000));
        provider.push(text_response(&original, 4000, 8000));
        provider.push_error(cursor_server::Error::Provider(
            "context_length_exceeded".into(),
        ));
        let mut truncated = text_response("UNCOMMITTED_TRUNCATED_SUMMARY", 100, 16000);
        *truncated.last_mut().unwrap() = ModelEvent::Done(FinishReason::Length);
        provider.push(truncated);
        for _ in 0..16 {
            provider.push(text_response("DURABLE_SUMMARY", 100, 20));
        }
        let assets = PromptAssets::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"),
        )
        .unwrap();
        let registry = TransportRegistry::new(
            store.clone(),
            Arc::new(provider.clone()),
            PromptCompiler::new(assets),
        );
        let first = run(
            &registry,
            "retry-first",
            user_request("cap-retry", "u1", "ORIGINAL_GOAL", &model.model_hash, None),
        )
        .await;
        let second = run(
            &registry,
            "retry-second",
            user_request(
                "cap-retry",
                "u2",
                "CURRENT_REQUEST",
                &model.model_hash,
                first.checkpoints.last().cloned(),
            ),
        )
        .await;
        assert!(
            matches!(second.end_stream, EndStream::Success {}),
            "{:?}",
            second.end_stream
        );
        assert_eq!(second.summary_completed, 1);
        let requests = provider.requests();
        let summaries = requests
            .iter()
            .filter(|request| request.prompt.tools.is_empty())
            .collect::<Vec<_>>();
        assert!(summaries.len() >= 3, "retry splits the rejected transcript");
        assert!(
            serde_json::to_string(&summaries[1].history).unwrap().len()
                < serde_json::to_string(&summaries[0].history).unwrap().len()
        );
        for request in &summaries {
            let output = request.model.max_output_tokens.unwrap();
            assert!(
                output > 0 && output < 16_384,
                "small window limits the configured 64K cap"
            );
        }
        let mut restored = String::new();
        for request in &summaries[1..] {
            for message in &request.history {
                if !message.message_id.starts_with("compaction:transcript:") {
                    continue;
                }
                let ProjectedContent::Parts(parts) = &message.content else {
                    panic!("transcript parts")
                };
                if matches!(&parts[0], ContentPart::Text { text } if text.starts_with("Historical message Assistant"))
                {
                    for part in &parts[1..] {
                        if let ContentPart::Text { text } = part {
                            restored.push_str(text);
                        }
                    }
                }
            }
        }
        assert_eq!(
            restored, original,
            "successful batches cover the complete original answer exactly once"
        );
        let retained = serde_json::to_string(
            &store
                .load_current_messages(&ConversationId::new("cap-retry"))
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(retained.contains("DURABLE_SUMMARY") && retained.contains("CURRENT_REQUEST"));
        assert!(!retained.contains("UNCOMMITTED_TRUNCATED_SUMMARY"));
    }
}

#[tokio::test]
async fn successful_batch_restores_retries_for_a_later_truncated_batch() {
    let (_directory, store) = fixtures::temp_store().await;
    let model = store
        .create_model(&windowed_model("later-retry", Some(100_000)))
        .await
        .unwrap();
    let provider = fake_provider::FakeProvider::default();
    let original = format!("COMPLETED_TASK {}", "0123456789".repeat(40_000));
    provider.push(text_response(&original, 150_000, 100_000));
    let mut truncated = text_response("UNCOMMITTED_TRUNCATION", 100, 16_384);
    *truncated.last_mut().unwrap() = ModelEvent::Done(FinishReason::Length);
    for _ in 0..3 {
        provider.push(truncated.clone());
    }
    provider.push(text_response("FIRST_CONFIRMED_SUMMARY", 100, 20));
    provider.push(truncated);
    for _ in 0..128 {
        provider.push(text_response("COMPLETE_DURABLE_SUMMARY", 100, 20));
    }
    let assets =
        PromptAssets::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt/cursor"))
            .unwrap();
    let registry = TransportRegistry::new(
        store.clone(),
        Arc::new(provider.clone()),
        PromptCompiler::new(assets),
    );
    let first = run(
        &registry,
        "later-first",
        user_request(
            "later-cap-retry",
            "u1",
            "ORIGINAL_GOAL",
            &model.model_hash,
            None,
        ),
    )
    .await;
    let second = run(
        &registry,
        "later-second",
        user_request(
            "later-cap-retry",
            "u2",
            "CURRENT_REQUEST",
            &model.model_hash,
            first.checkpoints.last().cloned(),
        ),
    )
    .await;
    assert!(
        matches!(second.end_stream, EndStream::Success {}),
        "{:?}",
        second.end_stream
    );
    assert_eq!(second.summary_completed, 1);
    let requests = provider.requests();
    let summaries = requests
        .iter()
        .filter(|request| request.prompt.tools.is_empty())
        .collect::<Vec<_>>();
    assert!(summaries.len() > 6);
    for request in &summaries[4..=5] {
        assert!(serde_json::to_string(&request.history)
            .unwrap()
            .contains("FIRST_CONFIRMED_SUMMARY"));
    }
    let mut restored = String::new();
    for (index, request) in summaries.iter().enumerate() {
        if [0, 1, 2, 4].contains(&index) {
            continue;
        }
        for message in &request.history {
            if !message.message_id.starts_with("compaction:transcript:") {
                continue;
            }
            let ProjectedContent::Parts(parts) = &message.content else {
                panic!("transcript parts")
            };
            if matches!(&parts[0], ContentPart::Text { text } if text.starts_with("Historical message Assistant"))
            {
                for part in &parts[1..] {
                    if let ContentPart::Text { text } = part {
                        restored.push_str(text);
                    }
                }
            }
        }
    }
    assert_eq!(restored, original);
    let retained = serde_json::to_string(
        &store
            .load_current_messages(&ConversationId::new("later-cap-retry"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(retained.contains("COMPLETE_DURABLE_SUMMARY") && retained.contains("CURRENT_REQUEST"));
    assert!(!retained.contains("UNCOMMITTED_TRUNCATION"));
}
