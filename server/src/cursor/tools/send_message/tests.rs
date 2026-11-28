use super::*;
use prost::Message;
use serde_json::{json, Value};

fn call(arguments: Value) -> ToolCall {
    ToolCall {
        index: 0,
        model_call_id: "model".into(),
        arguments_text: arguments.to_string(),
        argument_error: None,
        call_id: "project-message".into(),
        name: "SendMessage".into(),
        arguments,
    }
}

#[test]
fn native_text_limits_use_javascript_trimming_and_utf16_units() {
    for message in ["", " \n\t", "\u{feff}", &"😀".repeat(50_001)] {
        assert!(
            arguments(&call(json!({"message":message}))).is_err(),
            "{message:?}"
        );
    }
    for message in ["\u{85}", &"😀".repeat(50_000), "\u{feff}visible\u{feff}"] {
        assert!(arguments(&call(json!({"message":message}))).is_ok());
    }
    let args = arguments(&call(json!({"message":"\u{feff} visible \u{feff}"}))).unwrap();
    let Some(pb::send_message_args::Message::Text(text)) = args.message else {
        panic!("text")
    };
    assert_eq!(text.content, "visible");
}

#[test]
fn short_links_follow_native_code_and_whitespace_rules() {
    let id = "bc-12345678-1234-1234-1234-123456789abc";
    for text in [
        "````\n[worker](bc-unknown)\n```".into(),
        "~~~~\n[worker](bc-unknown)\n~~~".into(),
        "`[worker](bc-unknown)`".into(),
        format!("[worker]({id}\u{feff})"),
        format!("[worker]({id}#changes)"),
        format!("[worker]({id}/desktop)"),
        format!("[worker]({id}#file=src/main.rs&startLine=3)"),
        "[docs](https://example.invalid/)".into(),
    ] {
        assert!(links::validate(&text).is_ok(), "{text:?}");
    }
    for text in [
        "[worker](bc-unknown)".into(),
        format!("[worker]({id}\u{85})"),
        format!("[worker]({id}?hidden=1)"),
        format!("[worker]({id}/desktop#changes)"),
        format!("[worker]({id}#unknown)"),
        "`unclosed\n[worker](bc-unknown)".into(),
    ] {
        assert!(links::validate(&text).is_err(), "{text:?}");
    }
}

#[test]
fn validation_and_root_restrictions_keep_the_native_error_card() {
    for (input, root) in [
        (json!({}), true),
        (json!({"message":4}), true),
        (json!({"message":"ok","reply_to":"unsupported"}), true),
        (json!({"message":" "}), true),
        (json!({"message":"[bad](bc-unknown)"}), true),
        (json!({"message":"ok"}), false),
    ] {
        let call = call(input);
        let completion =
            super::super::validation_failure(&call, complete(&call, root).unwrap_err().to_string());
        assert!(completion.messages.is_empty());
        let completion = completion.completion.unwrap();
        assert!(completion.result().is_error);
        let encoded = completion.tool_call().encode_to_vec();
        let wire = pb::ToolCall::decode(encoded.as_slice()).unwrap();
        let Some(pb::tool_call::Tool::SendMessageToolCall(card)) = wire.tool else {
            panic!("typed card")
        };
        assert!(matches!(
            card.result.and_then(|r| r.result),
            Some(pb::send_message_result::Result::Error(_))
        ));
    }
}
