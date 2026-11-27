//! Owns Cursor goal state transitions and committed checkpoint projection.
mod action;
mod admission;
mod tools;

use std::collections::{HashMap, HashSet};

use super::protocol::proto::agent::v1 as pb;
use crate::{
    model::{CanonicalMessage, MessageContent},
    Result,
};

pub(crate) use action::prepare_action;
pub(crate) use admission::{GoalAdmissions, GoalGuard};
pub(crate) use tools::{execute, is_goal_tool};

#[derive(Clone, Default)]
pub(crate) struct GoalProjection {
    seen: HashSet<String>,
}

impl GoalProjection {
    pub(crate) fn seed(&mut self, messages: &[CanonicalMessage]) {
        self.seen
            .extend(messages.iter().map(|message| message.message_id.clone()));
    }

    pub(crate) fn apply(
        &mut self,
        state: &mut Option<pb::GoalState>,
        messages: &[CanonicalMessage],
    ) -> Result<()> {
        let mut calls = HashMap::new();
        for message in messages {
            if let MessageContent::Assistant { tool_calls, .. } = &message.content {
                for call in tool_calls {
                    calls.insert(&call.call_id, &call.name);
                }
            }
            if !self.seen.insert(message.message_id.clone()) {
                continue;
            }
            match &message.content {
                MessageContent::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                    if let Some(goal) = state.as_mut() {
                        goal.idle_continuations_without_tool_calls = 0;
                    }
                }
                MessageContent::ToolResult(result)
                    if !result.is_error
                        && calls
                            .get(&result.call_id)
                            .is_some_and(|name| is_goal_tool(name)) =>
                {
                    // The canonical result is committed before this projection runs.
                    // Replaying an old result must never undo a later client pause/clear.
                    *state = Some(serde_json::from_str::<GoalResult>(&result.content)?.goal);
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct GoalResult {
    status: String,
    goal: pb::GoalState,
}

fn owns(goal: &pb::GoalState, conversation: &str, session: Option<&str>) -> bool {
    match goal.agent_session_id.as_deref() {
        Some(owner) => !owner.is_empty() && Some(owner) == session,
        None => goal.conversation_id == conversation,
    }
}

fn accrue(goal: &mut pb::GoalState, now: u64, active: bool) {
    let elapsed = goal
        .last_accrued_at_ms
        .map_or(0, |last| now.saturating_sub(last));
    goal.active_duration_ms = Some(goal.active_duration_ms.unwrap_or(0).saturating_add(elapsed));
    goal.last_accrued_at_ms = active.then_some(now);
}
