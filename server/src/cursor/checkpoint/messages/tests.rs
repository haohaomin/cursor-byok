//! Verifies stable checkpoint Message encoding and recovery behavior.
use std::collections::HashSet;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};

use crate::model::{
    project_messages, CanonicalMessage, ContentPart, MessageContent, ProjectedContent,
    ProjectedMessage, ProviderReplayState, Role, ToolCall, ToolResultContent, ToolRoundAssistant,
};

use super::{decode, decode_pending, encode::wire_message, staged_tool_round};

#[test]
fn pending_tool_round_is_one_complete_assistant_message_and_round_trips() {
    let replay_state = ProviderReplayState {
        provider_kind: "anthropic".into(),
        value: json!({"blocks":[{"type":"thinking","thinking":"why","signature":"sig"}]}),
    };
    let assistant = ToolRoundAssistant {
        text: "before tools".into(),
        thinking: "why".into(),
        model_call_id: "model-call".into(),
        replay_state: Some(replay_state.clone()),
    };
    let calls = vec![
        ToolCall {
            index: 0,
            call_id: "a".into(),
            model_call_id: "model-call".into(),
            name: "Read".into(),
            arguments_text: r#"{"path":"/a"}"#.into(),
            arguments: json!({"path":"/a"}),
            argument_error: Some("Read arguments are not valid JSON".into()),
        },
        ToolCall {
            index: 1,
            call_id: "b".into(),
            model_call_id: "model-call".into(),
            name: "Grep".into(),
            arguments_text: r#"{"pattern":"x"}"#.into(),
            arguments: json!({"pattern":"x"}),
            argument_error: None,
        },
    ];
    let pending = staged_tool_round(
        &assistant,
        &calls,
        "claude",
        &["Read".into(), "Grep".into()],
        &HashSet::new(),
        42,
    )
    .unwrap();
    let wire: Value = serde_json::from_str(&pending).unwrap();
    assert_eq!(wire["id"], "1");
    assert_eq!(
        wire["providerOptions"]["cursor"]["pendingToolExecutionContracts"]["a"]["toolIdentifier"],
        "READ"
    );
    assert_eq!(
        wire["providerOptions"]["cursor"]["pendingToolExecutionContracts"]["a"]["argumentError"],
        "Read arguments are not valid JSON"
    );
    assert_eq!(wire["role"], "assistant");
    assert_eq!(
        wire["providerOptions"]["cursor"]["pendingToolExecutionContracts"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        wire["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part["type"] == "tool-call")
            .count(),
        2
    );

    let recovered = decode_pending(&pending).unwrap();
    assert_eq!(recovered.assistant.replay_state, Some(replay_state));
    assert_eq!(recovered.calls.len(), 2);
    assert_eq!(recovered.calls[0].call_id, "a");
    assert_eq!(
        recovered.calls[0].argument_error.as_deref(),
        Some("Read arguments are not valid JSON")
    );
    assert_eq!(recovered.calls[1].call_id, "b");
}

#[test]
fn cursor_wire_ids_are_projection_metadata_not_internal_message_ids() {
    let assistant = ProjectedMessage {
        message_id: "internal-assistant-id".into(),
        role: Role::Assistant,
        content: ProjectedContent::Assistant {
            text: "done".into(),
            thinking: String::new(),
            replay_state: None,
            calls: Vec::new(),
        },
    };
    let result = ProjectedMessage {
        message_id: "internal-result-id".into(),
        role: Role::Tool,
        content: ProjectedContent::ToolResult(ToolResultContent {
            call_id: "call-1".into(),
            name: "Read".into(),
            content: "ok".into(),
            is_error: false,
            image: None,
            provider_parts: Vec::new(),
        }),
    };

    assert_eq!(wire_message(&assistant, "model", None).unwrap()["id"], "1");
    assert_eq!(
        wire_message(&result, "model", None).unwrap()["id"],
        "call-1"
    );
}

#[test]
fn runtime_wire_identity_survives_checkpoint_hydration() {
    let wire = json!({
        "role": "user",
        "id": "runtime:subagent-completed:child-id",
        "content": "child completed",
    });
    let message = decode(
        serde_json::to_vec(&wire).unwrap().as_slice(),
        "cursor-root:blob-id:19".into(),
    )
    .unwrap();

    assert_eq!(message.message_id, "runtime:subagent-completed:child-id");
    assert_eq!(
        message.runtime_event_id.as_deref(),
        Some("subagent-completed:child-id")
    );
}

#[test]
fn request_context_identity_survives_checkpoint_hydration() {
    let wire = json!({
        "role": "user",
        "id": "request-context:digest",
        "content": "<rules>current rules</rules>",
    });
    let message = decode(
        serde_json::to_vec(&wire).unwrap().as_slice(),
        "cursor-root:blob-id:20".into(),
    )
    .unwrap();

    assert_eq!(message.message_id, "request-context:digest");
    assert_eq!(message.origin, crate::model::Origin::Prompt);
}

#[test]
fn cursor_user_image_uses_image_field() {
    let wire = json!({
        "role": "user",
        "id": "user-image",
        "content": [
            {"type":"text", "text":"look"},
            {"type":"image", "image":"AQID", "mimeType":"image/png"},
        ],
    });
    let message = decode(
        serde_json::to_vec(&wire).unwrap().as_slice(),
        "cursor-root:user-image".into(),
    )
    .unwrap();
    assert!(matches!(
        &message.content,
        MessageContent::Parts { parts }
            if parts[1] == ContentPart::Image {
                mime_type: "image/png".into(),
                data: vec![1, 2, 3],
            }
    ));

    let projected = project_messages(&[message]).unwrap();
    let encoded = wire_message(&projected[0], "model", None).unwrap();
    assert_eq!(encoded["content"][1]["image"], "AQID");
    assert!(encoded["content"][1].get("data").is_none());
}

#[test]
fn repeated_cursor_wire_ids_do_not_merge_distinct_tool_rounds() {
    fn assistant(call_id: &str, internal_id: &str) -> CanonicalMessage {
        let wire = json!({
            "role": "assistant",
            "id": "1",
            "content": [{
                "type": "tool-call",
                "toolCallId": call_id,
                "toolName": "Read",
                "args": {"path": format!("/{call_id}")},
            }],
        });
        decode(
            serde_json::to_vec(&wire).unwrap().as_slice(),
            internal_id.into(),
        )
        .unwrap()
    }
    fn result(call_id: &str, internal_id: &str) -> CanonicalMessage {
        let wire = json!({
            "role": "tool",
            "id": call_id,
            "content": [{
                "type": "tool-result",
                "toolCallId": call_id,
                "toolName": "Read",
                "result": "ok",
            }],
        });
        decode(
            serde_json::to_vec(&wire).unwrap().as_slice(),
            internal_id.into(),
        )
        .unwrap()
    }

    let messages = vec![
        assistant("a", "cursor-root:a"),
        result("a", "cursor-root:a-result"),
        assistant("b", "cursor-root:b"),
        result("b", "cursor-root:b-result"),
    ];
    assert_ne!(messages[0].message_id, messages[2].message_id);
    let projected = project_messages(&messages).unwrap();
    assert_eq!(projected.len(), 4);
    assert!(matches!(
        &projected[0].content,
        ProjectedContent::Assistant { calls, .. } if calls[0].call_id == "a"
    ));
    assert!(matches!(
        &projected[2].content,
        ProjectedContent::Assistant { calls, .. } if calls[0].call_id == "b"
    ));
}

#[test]
fn opaque_cursor_reasoning_signature_round_trips_without_decoding() {
    let signature = "opaque-url-safe_signature-value";
    let wire = json!({
        "role": "assistant",
        "id": "1",
        "content": [{"type":"reasoning", "text":"", "signature":signature}],
    });
    let message = decode(
        serde_json::to_vec(&wire).unwrap().as_slice(),
        "cursor-root:opaque".into(),
    )
    .unwrap();
    let MessageContent::Assistant { replay_state, .. } = &message.content else {
        panic!("expected assistant");
    };
    assert_eq!(
        replay_state.as_ref().unwrap().provider_kind,
        "cursor_opaque"
    );
    let projected = project_messages(&[message]).unwrap();
    let encoded = wire_message(&projected[0], "model", None).unwrap();
    assert_eq!(encoded["content"][0]["signature"], signature);
}

#[test]
fn multiple_opaque_cursor_reasoning_signatures_round_trip_in_order() {
    let wire = json!({
        "role": "assistant",
        "id": "1",
        "content": [
            {"type":"reasoning", "text":"first", "signature":"opaque-1"},
            {"type":"reasoning", "text":"second", "signature":"opaque-2"},
            {"type":"text", "text":"answer"}
        ]
    });
    let message = decode(
        serde_json::to_vec(&wire).unwrap().as_slice(),
        "cursor-root:opaque-sequence".into(),
    )
    .unwrap();
    let MessageContent::Assistant {
        thinking,
        replay_state,
        ..
    } = &message.content
    else {
        panic!("expected assistant");
    };
    assert_eq!(thinking, "firstsecond");
    assert_eq!(
        replay_state.as_ref().unwrap().value["signatures"],
        json!(["opaque-1", "opaque-2"])
    );
    assert_eq!(
        replay_state.as_ref().unwrap().value["texts"],
        json!(["first", "second"])
    );

    let projected = project_messages(&[message]).unwrap();
    let encoded = wire_message(&projected[0], "model", None).unwrap();
    let content = encoded["content"].as_array().unwrap();
    assert_eq!(
        &content[..2],
        json!([
            {"type":"reasoning", "text":"first", "signature":"opaque-1", "providerOptions":{"cursor":{"modelName":"model"}}},
            {"type":"reasoning", "text":"second", "signature":"opaque-2", "providerOptions":{"cursor":{"modelName":"model"}}}
        ])
        .as_array()
        .unwrap()
    );
}

#[test]
fn merged_opaque_signatures_keep_thinking_when_boundaries_are_unknown() {
    let message = ProjectedMessage {
        message_id: "assistant".into(),
        role: Role::Assistant,
        content: ProjectedContent::Assistant {
            text: "answer".into(),
            thinking: "combined".into(),
            replay_state: Some(ProviderReplayState {
                provider_kind: "cursor_opaque".into(),
                value: json!({"signatures":["opaque-1", "opaque-2"]}),
            }),
            calls: Vec::new(),
        },
    };
    let encoded = wire_message(&message, "model", None).unwrap();
    assert_eq!(
        encoded["content"],
        json!([
            {"type":"reasoning", "text":"combined", "signature":"opaque-1", "providerOptions":{"cursor":{"modelName":"model"}}},
            {"type":"reasoning", "text":"", "signature":"opaque-2", "providerOptions":{"cursor":{"modelName":"model"}}},
            {"type":"text", "text":"answer"}
        ])
    );
}

#[test]
fn multiple_enveloped_signatures_merge_provider_blocks_in_order() {
    let envelope = |signature: &str| {
        format!(
            "{}{}",
            super::REPLAY_ENVELOPE_PREFIX,
            STANDARD.encode(
                serde_json::to_vec(&json!({
                    "provider_kind": "anthropic",
                    "value": {"blocks": [{"type":"thinking", "signature": signature}]}
                }))
                .unwrap()
            )
        )
    };
    let wire = json!({
        "role": "assistant",
        "id": "1",
        "content": [
            {"type":"reasoning", "text":"first", "signature": envelope("sig-1")},
            {"type":"reasoning", "text":"second", "signature": envelope("sig-2")},
            {"type":"text", "text":"answer"}
        ]
    });
    let message = decode(
        serde_json::to_vec(&wire).unwrap().as_slice(),
        "cursor-root:enveloped-sequence".into(),
    )
    .unwrap();
    let MessageContent::Assistant {
        thinking,
        replay_state,
        ..
    } = message.content
    else {
        panic!("expected assistant");
    };
    assert_eq!(thinking, "firstsecond");
    assert_eq!(
        replay_state.unwrap().value["blocks"],
        json!([
            {"type":"thinking", "signature":"sig-1"},
            {"type":"thinking", "signature":"sig-2"}
        ])
    );
}
