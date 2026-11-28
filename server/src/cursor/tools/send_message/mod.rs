//! Produces the project's user-visible message card without external execution.
mod links;
#[cfg(test)]
mod tests;

use crate::{
    cursor::protocol::proto::agent::v1 as pb,
    model::{ToolCall, ToolResult},
    Error, Result,
};

use super::{runtime::now_ms, tool_call_result::ToolCompletion};

pub(crate) fn arguments(call: &ToolCall) -> Result<pb::SendMessageArgs> {
    let values = call
        .arguments
        .as_object()
        .ok_or_else(|| Error::Protocol("SendMessage arguments must be an object".into()))?;
    if let Some(name) = values.keys().find(|name| name.as_str() != "message") {
        return Err(Error::Protocol(format!(
            "Unknown SendMessage parameter: {name}"
        )));
    }
    let text = values
        .get("message")
        .and_then(|value| value.as_str())
        .ok_or_else(|| Error::Protocol("SendMessage message must be a string".into()))?
        .trim_matches(js_whitespace);
    if text.is_empty() || text.encode_utf16().count() > 100_000 {
        return Err(Error::Protocol(
            "SendMessage message must contain 1 to 100000 UTF-16 code units after trimming".into(),
        ));
    }
    links::validate(text)?;
    Ok(pb::SendMessageArgs {
        message: Some(pb::send_message_args::Message::Text(pb::SendMessageText {
            content: text.into(),
        })),
    })
}

pub(crate) fn complete(call: &ToolCall, project_root: bool) -> Result<ToolCompletion> {
    if !project_root {
        return Err(Error::Protocol(
            "SendMessage is available only in the root project conversation".into(),
        ));
    }
    let args = arguments(call)?;
    let timestamp = now_ms();
    Ok(finish(
        call,
        Some(args),
        "Message sent to user.".into(),
        false,
        pb::send_message_result::Result::Success(pb::SendMessageSuccess {
            timestamp,
            message_id: call.call_id.clone(),
        }),
        timestamp,
    ))
}

pub(crate) fn failure(call: &ToolCall, error: String) -> ToolCompletion {
    finish(
        call,
        arguments(call).ok(),
        error.clone(),
        true,
        pb::send_message_result::Result::Error(pb::SendMessageError { error }),
        now_ms(),
    )
}

fn finish(
    call: &ToolCall,
    args: Option<pb::SendMessageArgs>,
    content: String,
    is_error: bool,
    result: pb::send_message_result::Result,
    started_ms: u64,
) -> ToolCompletion {
    ToolCompletion::new(
        call,
        started_ms,
        ToolResult {
            call_id: call.call_id.clone(),
            content,
            is_error,
            image: None,
        },
        pb::tool_call::Tool::SendMessageToolCall(pb::SendMessageToolCall {
            args,
            result: Some(pb::SendMessageResult {
                result: Some(result),
            }),
        }),
    )
}

// ECMAScript WhiteSpace + LineTerminator, shared by trim() and /\s/.
fn js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}
