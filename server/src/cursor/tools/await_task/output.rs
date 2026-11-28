//! One canonical result and one Await card per completed observation.
use super::{super::tool_call_result::ToolCompletion, Wait};
use crate::{cursor::protocol::proto::agent::v1 as pb, model::ToolResult};

#[allow(clippy::too_many_arguments)]
pub(super) fn complete(
    id: &str,
    runtime_ms: u64,
    path: &str,
    length: u64,
    regex: bool,
    regex_match: Option<String>,
    exit_code: Option<i32>,
) -> pb::await_success::AwaitResult {
    pb::await_success::AwaitResult::Complete(pb::AwaitTaskComplete {
        task_id: id.into(),
        runtime_ms,
        output_file_path: path.into(),
        output_length: length,
        regex_requested: regex,
        regex_match,
        exit_code,
        wake_reason: None,
    })
}

pub(super) fn running(
    id: &str,
    runtime_ms: u64,
    path: &str,
    length: u64,
    regex: bool,
    regex_match: Option<String>,
) -> pb::await_success::AwaitResult {
    pb::await_success::AwaitResult::StillRunning(pb::AwaitTaskStillRunning {
        task_id: id.into(),
        runtime_ms,
        output_file_path: path.into(),
        output_length: length,
        regex_requested: regex,
        regex_match,
        wake_reason: None,
    })
}

pub(super) async fn subagent(
    wait: &Wait,
    response: std::result::Result<pb::exec_client_message::Message, String>,
) -> std::result::Result<pb::await_success::AwaitResult, String> {
    let pb::exec_client_message::Message::SubagentAwaitResult(result) = response? else {
        return Err("Expected SubagentAwaitResult from Cursor".into());
    };
    use pb::subagent_await_result::Result;
    let (_id, path, done) = match result.result {
        Some(Result::Complete(value)) => (value.agent_id, value.transcript_path, true),
        Some(Result::StillRunning(value)) => (value.agent_id, value.transcript_path, false),
        Some(Result::NotFound(value)) => {
            return Err(format!("Subagent not found: {}", value.agent_id))
        }
        Some(Result::Error(value)) => return Err(value.error),
        None => return Err("Cursor returned an empty SubagentAwaitResult".into()),
    };
    let path = path.unwrap_or_default();
    let length = if path.is_empty() {
        0
    } else {
        match wait.read(&path).await {
            Ok((message, response)) => {
                wait.queue(message).await;
                match super::receive(response)
                    .await
                    .and_then(super::shell::read_output)
                {
                    Ok((_, length)) => length,
                    Err(_) => 0,
                }
            }
            Err(_) => 0,
        }
    };
    let id = &wait.args.task_id;
    let elapsed = wait.started.elapsed().as_millis() as u64;
    Ok(if done {
        complete(id, elapsed, &path, length, false, None, None)
    } else {
        running(id, elapsed, &path, length, false, None)
    })
}

pub(super) fn completion(
    wait: &Wait,
    result: std::result::Result<pb::await_success::AwaitResult, String>,
) -> ToolCompletion {
    let (content, is_error, wire) = match result {
        Ok(result) => {
            let content = match &result {
                pb::await_success::AwaitResult::Complete(value) => serde_json::json!({
                    "task_id":value.task_id,"status":"completed","runtime_ms":value.runtime_ms,
                    "output_file_path":value.output_file_path,"output_length":value.output_length,
                    "regex_match":value.regex_match,"exit_code":value.exit_code,
                }),
                pb::await_success::AwaitResult::StillRunning(value) => serde_json::json!({
                    "task_id":value.task_id,"status":"still_running","runtime_ms":value.runtime_ms,
                    "output_file_path":value.output_file_path,"output_length":value.output_length,
                    "regex_match":value.regex_match,
                }),
            };
            (
                content.to_string(),
                false,
                pb::await_result::Result::Success(pb::AwaitSuccess {
                    await_result: Some(result),
                }),
            )
        }
        Err(error) => (
            error.clone(),
            true,
            pb::await_result::Result::Error(pb::AwaitError { error }),
        ),
    };
    ToolCompletion::new(
        &wait.call,
        wait.started_ms,
        ToolResult {
            call_id: wait.call.call_id.clone(),
            content,
            is_error,
            image: None,
        },
        pb::tool_call::Tool::AwaitToolCall(pb::AwaitToolCall {
            args: Some(wait.args.wire()),
            result: Some(pb::AwaitResult { result: Some(wire) }),
        }),
    )
}
