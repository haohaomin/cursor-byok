//! Selected custom roles must reach the provider and survive checkpoint recovery.
use super::*;
use cursor_server::model::{ContentPart, ModelRequest, ProjectedContent};

const WRITER: &str = "WRITER_ROLE: write documentation with runnable examples.";
const REVIEWER: &str = "REVIEWER_ROLE: find reproducible defects and cite source lines.";

fn role_run(
    id: &str,
    state: Option<pb::ConversationStateStructure>,
    selected: Option<&str>,
    prompt: &str,
) -> pb::AgentClientMessage {
    let mut message = run(id, state);
    let Some(pb::agent_client_message::Message::RunRequest(request)) = message.message.as_mut()
    else {
        unreachable!()
    };
    request.subagent_type_name = selected.map(str::to_string);
    let Some(pb::conversation_action::Action::UserMessageAction(action)) =
        request.action.as_mut().unwrap().action.as_mut()
    else {
        unreachable!()
    };
    action.request_context = Some(pb::RequestContext {
        custom_subagents: vec![
            pb::CustomSubagent {
                name: "docs-writer".into(),
                description: "Writes documentation".into(),
                prompt: prompt.into(),
                ..Default::default()
            },
            pb::CustomSubagent {
                name: "reviewer".into(),
                description: "Reviews code".into(),
                prompt: REVIEWER.into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    message
}

fn contexts(request: &ModelRequest) -> Vec<(&str, &str)> {
    request
        .history
        .iter()
        .filter_map(|message| {
            if !message.message_id.starts_with("request-context:") {
                return None;
            }
            let ProjectedContent::Parts(parts) = &message.content else {
                panic!("context parts")
            };
            let [ContentPart::Text { text }] = parts.as_slice() else {
                panic!("context text")
            };
            Some((message.message_id.as_str(), text.as_str()))
        })
        .collect()
}

#[tokio::test]
async fn only_the_selected_custom_role_reaches_the_provider() {
    for selected in [
        Some("docs-writer"),
        Some("reviewer"),
        None,
        Some("unknown"),
        Some("explore"),
    ] {
        let (_directory, store) = fixtures::temp_store().await;
        let provider = fake_provider::FakeProvider::default();
        let registry = registry(&store, &provider);
        complete(
            &registry,
            &provider,
            "selected-role",
            &[],
            role_run("u1", None, selected, WRITER),
        )
        .await;
        let requests = provider.requests();
        let text = serde_json::to_string(&requests[0]).unwrap();
        assert_eq!(
            text.contains(WRITER),
            selected == Some("docs-writer"),
            "selected {selected:?}"
        );
        assert_eq!(
            text.contains(REVIEWER),
            selected == Some("reviewer"),
            "selected {selected:?}"
        );
        assert!(!requests[0].prompt.instructions.contains("_ROLE:"));
        if selected == Some("docs-writer") {
            assert!(contexts(&requests[0]).last().unwrap().1.contains(WRITER));
        }
        registry.shutdown().await;
    }
}

#[tokio::test]
async fn role_updates_append_without_rewriting_history_and_survive_restart() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let mut state = None;
    for (index, prompt) in [WRITER, WRITER, "WRITER_ROLE_B: use short examples.", WRITER]
        .into_iter()
        .enumerate()
    {
        let registry = registry(&store, &provider);
        let id = format!("role-turn-{index}");
        let message = role_run(&id, state, Some("docs-writer"), prompt);
        state = Some(complete(&registry, &provider, &id, &[], message.clone()).await);
        // Retrying the same input must reuse the same context and runtime identity.
        if index == 0 {
            complete(&registry, &provider, "role-retry", &[], message).await;
            assert_eq!(
                provider.requests()[0].history,
                provider.requests()[1].history
            );
        }
        registry.shutdown().await;
    }
    let requests = provider.requests();
    for pair in requests.windows(2) {
        assert_eq!(pair[0].prompt.instructions, pair[1].prompt.instructions);
        assert_eq!(
            serde_json::to_value(&pair[0].prompt.tools).unwrap(),
            serde_json::to_value(&pair[1].prompt.tools).unwrap()
        );
        assert_eq!(pair[0].history, pair[1].history[..pair[0].history.len()]);
    }
    assert_eq!(
        contexts(&requests[2]).len(),
        1,
        "identical role context is not repeated"
    );
    let projected = contexts(requests.last().unwrap());
    assert_eq!(projected.len(), 3, "A → B → A must preserve each update");
    assert!(projected[0].1.contains(WRITER));
    assert!(projected[1].1.contains("WRITER_ROLE_B"));
    assert!(projected[2].1.contains(WRITER));
    let ids = projected
        .iter()
        .map(|(id, _)| *id)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 3);
    let history = &requests.last().unwrap().history;
    let context_index = history
        .iter()
        .position(|message| message.message_id == projected[2].0)
        .unwrap();
    assert!(history[context_index + 1]
        .message_id
        .starts_with("runtime:"));
}

#[tokio::test]
async fn custom_role_is_loaded_from_the_request_context_blob() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    let mut message = role_run("blob-role", None, Some("docs-writer"), WRITER);
    let Some(pb::agent_client_message::Message::RunRequest(request)) = message.message.as_mut()
    else {
        unreachable!()
    };
    let action = request.action.as_mut().unwrap();
    let Some(pb::conversation_action::Action::UserMessageAction(user)) = action.action.as_mut()
    else {
        unreachable!()
    };
    let blob = pb::RequestContextSubagentsPart {
        custom_subagents: user.request_context.take().unwrap().custom_subagents,
    }
    .encode_to_vec();
    let id = BlobId::digest(&blob).as_bytes().to_vec();
    action.request_context_parts = Some(pb::RequestContextPartReferences {
        subagents_blob_id: id.clone(),
        subagents_byte_length: blob.len() as u32,
        ..Default::default()
    });
    request
        .pre_fetched_blobs
        .push(pb::PreFetchedBlob { id, value: blob });
    complete(&registry, &provider, "blob-role", &[], message).await;
    assert!(contexts(&provider.requests()[0])
        .last()
        .unwrap()
        .1
        .contains(WRITER));
    registry.shutdown().await;
}

#[tokio::test]
async fn clearing_a_role_appends_context_without_inheriting_another_role() {
    for removal in ["blank-prompt", "removed-definition", "no-selection"] {
        check_role_removal(removal).await;
    }
}

async fn check_role_removal(removal: &str) {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    let first = complete(
        &registry,
        &provider,
        "clear-role-1",
        &[],
        role_run("u1", None, Some("docs-writer"), WRITER),
    )
    .await;
    let mut cleared = role_run("u2", Some(first), Some("docs-writer"), " \n");
    let Some(pb::agent_client_message::Message::RunRequest(request)) = cleared.message.as_mut()
    else {
        unreachable!()
    };
    if removal == "no-selection" {
        request.subagent_type_name = None;
    } else if removal == "removed-definition" {
        let Some(pb::conversation_action::Action::UserMessageAction(action)) =
            request.action.as_mut().unwrap().action.as_mut()
        else {
            unreachable!()
        };
        action
            .request_context
            .as_mut()
            .unwrap()
            .custom_subagents
            .retain(|agent| agent.name != "docs-writer");
    }
    complete(&registry, &provider, "clear-role-2", &[], cleared).await;
    let requests = provider.requests();
    let projected = contexts(&requests[1]);
    assert_eq!(projected.len(), 2);
    assert!(!projected[1].1.contains(WRITER));
    assert!(!projected[1].1.contains(REVIEWER));
    assert!(
        projected[1]
            .1
            .contains("<custom_subagent active=\"false\">"),
        "missing inactive role state after {removal}"
    );
    assert!(projected[1]
        .1
        .contains("Previous custom subagent instructions no longer apply."));
    assert_eq!(
        requests[0].history,
        requests[1].history[..requests[0].history.len()]
    );
    registry.shutdown().await;
}

#[tokio::test]
async fn role_context_survives_automatic_compaction() {
    let (_directory, store) = fixtures::temp_store().await;
    let provider = fake_provider::FakeProvider::default();
    let registry = registry(&store, &provider);
    let first = complete(
        &registry,
        &provider,
        "compact-role-1",
        &[],
        role_run("u1", None, Some("docs-writer"), WRITER),
    )
    .await;
    let second = complete(
        &registry,
        &provider,
        "compact-role-2",
        &[],
        role_run("u2", Some(first), Some("docs-writer"), "LATEST_WRITER_ROLE"),
    )
    .await;
    provider.push_error(cursor_server::Error::Provider(
        "context_length_exceeded".into(),
    ));
    provider.push(vec![
        ModelEvent::Start {
            model_call_id: "summary".into(),
        },
        ModelEvent::TextStart,
        ModelEvent::TextDelta("Previous work is complete; continue the next task.".into()),
        ModelEvent::TextEnd,
        ModelEvent::Done(FinishReason::Stop),
    ]);
    complete(
        &registry,
        &provider,
        "compact-role-3",
        &[],
        role_run(
            "u3",
            Some(second),
            Some("docs-writer"),
            "LATEST_WRITER_ROLE",
        ),
    )
    .await;
    let requests = provider.requests();
    assert_eq!(
        requests.len(),
        5,
        "overflow, summary, and continuation were exercised"
    );
    let retained = contexts(requests.last().unwrap());
    assert_eq!(retained.len(), 1);
    assert!(retained[0].1.contains("LATEST_WRITER_ROLE"));
    assert!(!retained[0].1.contains(WRITER));
    assert_eq!(
        requests[0].prompt.instructions,
        requests.last().unwrap().prompt.instructions
    );
    registry.shutdown().await;
}
