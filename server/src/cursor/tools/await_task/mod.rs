//! Observe existing client work; awaiting never starts or resumes a task.
mod output;
mod shell;
#[cfg(test)]
mod tests;

pub(super) use output::validation_failure;

use serde_json::Value;
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};
use tokio::{sync::oneshot, time::Instant};

use super::{
    runtime::{now_ms, CursorToolRuntime, ExecContext},
    tool_call_dispatch::ToolStart,
    tool_call_result::ToolResultSender,
};
use crate::{cursor::protocol::proto::agent::v1 as pb, model::ToolCall, Error, Result};

const MAX_WAIT_MS: u64 = 7_140_000;

pub(crate) struct Arguments {
    task_id: String,
    wait_ms: u32,
    pattern: Option<String>,
}

impl Arguments {
    pub(crate) fn wire(&self) -> pb::AwaitArgs {
        pb::AwaitArgs {
            task_id: self.task_id.clone(),
            block_until_ms: Some(self.wait_ms),
            regex: self.pattern.clone(),
        }
    }

    fn is_shell(&self) -> bool {
        !self.task_id.is_empty() && self.task_id.bytes().all(|b| b.is_ascii_digit())
    }
}

pub(crate) fn arguments(call: &ToolCall) -> Result<Arguments> {
    let values = call
        .arguments
        .as_object()
        .ok_or_else(|| Error::Protocol("Await arguments must be an object".into()))?;
    if let Some(name) = values
        .keys()
        .find(|name| !matches!(name.as_str(), "task_id" | "block_until_ms" | "pattern"))
    {
        return Err(Error::Protocol(format!("Unknown Await parameter: {name}")));
    }
    let task_id = match values.get("task_id") {
        None | Some(Value::Null) => "",
        Some(Value::String(id)) => id.trim(),
        _ => return Err(Error::Protocol("Await task_id must be a string".into())),
    };
    let task_id = if task_id.eq_ignore_ascii_case("none") {
        ""
    } else {
        task_id
    }
    .to_string();
    let wait_ms = match values.get("block_until_ms") {
        None | Some(Value::Null) => 30_000,
        Some(value) => {
            let value = value
                .as_f64()
                .ok_or_else(|| Error::Protocol("Await block_until_ms must be a number".into()))?;
            if !value.is_finite() || value < 0.0 {
                30_000
            } else {
                value.floor().min(MAX_WAIT_MS as f64) as u32
            }
        }
    };
    let pattern = match values.get("pattern") {
        None | Some(Value::Null) => None,
        Some(Value::String(pattern)) => Some(pattern.clone()),
        _ => return Err(Error::Protocol("Await pattern must be a string".into())),
    };
    if task_id.is_empty() && wait_ms == 0 {
        return Err(Error::Protocol(
            "Await without a task_id requires block_until_ms greater than zero".into(),
        ));
    }
    let args = Arguments {
        task_id,
        wait_ms,
        pattern,
    };
    if !args.task_id.is_empty()
        && !args.is_shell()
        && args.pattern.as_ref().is_some_and(|p| !p.is_empty())
    {
        return Err(Error::Protocol(
            "Await pattern is only supported for Shell tasks".into(),
        ));
    }
    Ok(args)
}

type Response = oneshot::Receiver<std::result::Result<pb::exec_client_message::Message, String>>;

struct Wait {
    call: ToolCall,
    context: ExecContext,
    args: Arguments,
    runtime: CursorToolRuntime,
    cancelled: Arc<AtomicBool>,
    started_ms: u64,
    started: Instant,
}

impl Wait {
    async fn request(
        &self,
        message: pb::exec_server_message::Message,
    ) -> Result<(pb::AgentServerMessage, Response)> {
        let (id, response) = self
            .runtime
            .reserve_await(&self.call, &self.context, &self.cancelled)
            .await?;
        let exec_id = match &message {
            pb::exec_server_message::Message::ReadArgs(_) => {
                format!("await-read-{}-{id}", self.call.call_id)
            }
            _ => self.call.call_id.clone(),
        };
        Ok((
            pb::AgentServerMessage {
                message: Some(pb::agent_server_message::Message::ExecServerMessage(
                    pb::ExecServerMessage {
                        id,
                        exec_id,
                        message: Some(message),
                        accept_hook_additional_contexts: Some(false),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
            response,
        ))
    }

    async fn read(&self, path: &str) -> Result<(pb::AgentServerMessage, Response)> {
        self.request(pb::exec_server_message::Message::ReadArgs(pb::ReadArgs {
            path: path.into(),
            tool_call_id: self.call.call_id.clone(),
            ..Default::default()
        }))
        .await
    }

    async fn queue(&self, message: pb::AgentServerMessage) {
        let Some(pb::agent_server_message::Message::ExecServerMessage(exec)) =
            message.message.as_ref()
        else {
            unreachable!()
        };
        self.runtime.queue_exec(exec.id, message).await;
    }
}

async fn receive(
    response: Response,
) -> std::result::Result<pb::exec_client_message::Message, String> {
    response
        .await
        .map_err(|_| "Await client execution was cancelled".to_string())?
}

pub(super) async fn start(
    runtime: &CursorToolRuntime,
    results: &ToolResultSender,
    call: &ToolCall,
    context: &ExecContext,
) -> Result<ToolStart> {
    let args = arguments(call)?;
    if !args.task_id.is_empty()
        && !args.is_shell()
        && (context.subagents_disabled || !context.allow_subagents)
    {
        return Err(Error::Protocol(
            "Subagent waiting is unavailable in this context".into(),
        ));
    }
    let shell = args
        .is_shell()
        .then(|| shell::Shell::new(&args, context))
        .transpose()?;
    let cancelled = runtime.register_worker(&call.call_id).await;
    let wait = Wait {
        call: call.clone(),
        context: context.clone(),
        args,
        runtime: runtime.clone(),
        cancelled: cancelled.clone(),
        started_ms: now_ms(),
        started: Instant::now(),
    };
    let first = if let Some(shell) = &shell {
        Some(wait.read(&shell.path).await?)
    } else if !wait.args.task_id.is_empty() {
        Some(
            wait.request(pb::exec_server_message::Message::SubagentAwaitArgs(
                pb::SubagentAwaitArgs {
                    agent_id: wait.args.task_id.clone(),
                    timeout_ms: wait.args.wait_ms,
                },
            ))
            .await?,
        )
    } else {
        None
    };
    let (messages, response) = match first {
        Some((message, response)) => (vec![message], Some(response)),
        None => (Vec::new(), None),
    };
    let sender = results.clone();
    let worker = tokio::spawn(async move {
        let result = if let Some(shell) = shell {
            shell
                .run(&wait, response.expect("Shell read response"))
                .await
        } else if let Some(response) = response {
            output::subagent(&wait, receive(response).await).await
        } else {
            tokio::time::sleep(Duration::from_millis(u64::from(wait.args.wait_ms))).await;
            Ok(output::complete(
                &wait.args.task_id,
                u64::from(wait.args.wait_ms),
                "",
                0,
                false,
                None,
                None,
            ))
        };
        let completion = output::completion(&wait, result);
        wait.runtime
            .finish_worker(&wait.call.call_id, &wait.cancelled, &sender, Ok(completion))
            .await;
    });
    runtime
        .attach_worker(&call.call_id, &cancelled, worker.abort_handle())
        .await;
    Ok(ToolStart {
        messages,
        completion: None,
    })
}
