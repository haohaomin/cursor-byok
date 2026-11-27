//! Executes local goal tools; the resulting snapshot becomes durable at commit.
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{accrue, owns, GoalResult};
use crate::cursor::{
    protocol::proto::agent::v1 as pb,
    tools::{
        codec,
        runtime::{now_ms, ExecContext},
        tool_call_result::ToolCompletion,
    },
};
use crate::{
    model::{ToolCall, ToolResult},
    Error, Result,
};

pub(crate) fn is_goal_tool(name: &str) -> bool {
    name.eq_ignore_ascii_case("CreateGoal") || name.eq_ignore_ascii_case("UpdateGoal")
}

pub(crate) fn execute(
    call: &ToolCall,
    context: &ExecContext,
    state: &mut Option<pb::GoalState>,
) -> Result<ToolCompletion> {
    let now = now_ms();
    let mut next = state.clone();
    let result = transition(call, context, &mut next, now);
    let mut rendered = codec::render_tool_call(call, false)?;
    match rendered.tool.as_mut() {
        Some(pb::tool_call::Tool::CreateGoalToolCall(tool)) => {
            tool.result = Some(pb::CreateGoalResult {
                result: Some(match &result {
                    Ok(()) => pb::create_goal_result::Result::Success(pb::CreateGoalSuccess {}),
                    Err(error) => pb::create_goal_result::Result::Error(pb::GoalError {
                        error: error.clone(),
                    }),
                }),
            })
        }
        Some(pb::tool_call::Tool::UpdateGoalToolCall(tool)) => {
            tool.result = Some(pb::UpdateGoalResult {
                result: Some(match &result {
                    Ok(()) => pb::update_goal_result::Result::Success(pb::UpdateGoalSuccess {
                        status: next.as_ref().expect("successful goal transition").status,
                    }),
                    Err(error) => pb::update_goal_result::Result::Error(pb::GoalError {
                        error: error.clone(),
                    }),
                }),
            })
        }
        _ => {
            return Err(Error::Protocol(
                "goal tool has no Cursor representation".into(),
            ))
        }
    }
    let is_error = result.is_err();
    let content = match result {
        Ok(()) => {
            *state = next;
            let goal = state.clone().expect("successful goal transition");
            serde_json::to_string(&GoalResult {
                status: if goal.status == pb::GoalStatus::Complete as i32 {
                    "complete"
                } else {
                    "active"
                }
                .into(),
                goal,
            })?
        }
        Err(error) => error,
    };
    Ok(ToolCompletion::new(
        call,
        now,
        ToolResult {
            call_id: call.call_id.clone(),
            content,
            is_error,
            image: None,
        },
        rendered.tool.expect("goal representation checked"),
    ))
}

fn transition(
    call: &ToolCall,
    context: &ExecContext,
    state: &mut Option<pb::GoalState>,
    now: u64,
) -> std::result::Result<(), String> {
    if context.conversation_id.is_empty() {
        return Err("Goal tools require a conversation ID".into());
    }
    if call.name.eq_ignore_ascii_case("CreateGoal") {
        let objective = call
            .arguments
            .get("objective")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("Goal objective must not be empty")?;
        // Tool IDs are unique only within a model round in the canonical store.
        let digest = Sha256::digest(
            format!(
                "{}\0{}\0{}",
                context.conversation_id, call.model_call_id, call.call_id
            )
            .as_bytes(),
        );
        let mut bytes: [u8; 16] = digest[..16].try_into().expect("SHA256 prefix");
        bytes[6] = (bytes[6] & 0x0f) | 0x50;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let goal_id = Uuid::from_bytes(bytes).to_string();
        if state.as_ref().is_some_and(|goal| {
            goal.goal_id == goal_id
                && owns(
                    goal,
                    &context.conversation_id,
                    context.agent_session_id.as_deref(),
                )
        }) {
            return Ok(());
        }
        *state = Some(pb::GoalState {
            conversation_id: context.conversation_id.clone(),
            goal_id,
            objective: objective.into(),
            status: pb::GoalStatus::Active as i32,
            agent_session_id: context.agent_session_id.clone(),
            active_duration_ms: Some(0),
            last_accrued_at_ms: Some(now),
            ..Default::default()
        });
    } else {
        let status = match call
            .arguments
            .get("status")
            .and_then(serde_json::Value::as_str)
        {
            Some("active") => pb::GoalStatus::Active,
            Some("complete") => pb::GoalStatus::Complete,
            _ => {
                return Err(
                    "Goal status must be active or complete; pausing is controlled by the user"
                        .into(),
                )
            }
        };
        let goal = state
            .as_mut()
            .filter(|goal| {
                owns(
                    goal,
                    &context.conversation_id,
                    context.agent_session_id.as_deref(),
                ) && goal.status != pb::GoalStatus::Cleared as i32
            })
            .ok_or("No goal exists in this session")?;
        accrue(goal, now, status == pb::GoalStatus::Active);
        goal.status = status as i32;
        if status == pb::GoalStatus::Active {
            goal.idle_continuations_without_tool_calls = 0;
        }
    }
    Ok(())
}
