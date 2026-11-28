//! Dispatches edit Tool calls.
//! Hidden read phase for file editing tools.

use crate::{model::ToolCall, Result};

use super::ToolStart;
use crate::cursor::tools::{
    codec, edit,
    runtime::{now_ms, CursorToolRuntime, ExecContext},
    tool_call_result,
};

pub(super) async fn start(
    runtime: &CursorToolRuntime,
    call: &ToolCall,
    context: &ExecContext,
) -> Result<ToolStart> {
    if let Some(plan) = &context.plan {
        if let Err(error) = plan.validate(&edit::path(call)?) {
            return Ok(ToolStart {
                messages: Vec::new(),
                completion: Some(tool_call_result::edit_failure(call, now_ms(), error)?),
            });
        }
    }
    let id = runtime.reserve_edit_read(call, context).await?;
    Ok(ToolStart {
        messages: vec![codec::edit_read_request(id, call)?],
        completion: None,
    })
}
