//! Admits client wakeups and projects one stable synthetic user turn per continuation.
use prost::Message;
use uuid::Uuid;

use super::{accrue, owns};
use crate::cursor::{protocol::proto::agent::v1 as pb, tools::runtime::now_ms};
use crate::{
    model::ConversationId,
    store::{BlobId, Store},
    Result,
};

/// Some(checkpoint) acknowledges a wakeup without starting another model Run.
pub(crate) async fn prepare_action(
    request: &mut pb::AgentRunRequest,
    store: &Store,
) -> Result<Option<pb::ConversationStateStructure>> {
    let continuation = matches!(
        request
            .action
            .as_ref()
            .and_then(|action| action.action.as_ref()),
        Some(pb::conversation_action::Action::GoalContinuationAction(_))
    );
    let Some(conversation_id) = request.conversation_id.as_deref() else {
        return Ok(continuation.then(|| request.conversation_state.clone().unwrap_or_default()));
    };
    let checkpoint_id = store
        .ensure_conversation(&ConversationId::new(conversation_id))
        .await?;
    let mut stored = store
        .cursor_checkpoint(checkpoint_id)
        .await?
        .map(|bytes| pb::ConversationStateStructure::decode(bytes.as_slice()))
        .transpose()?;
    if request.conversation_state.is_none()
        && stored
            .as_ref()
            .is_some_and(|state| state.goal_state.is_some())
    {
        request.conversation_state = stored.clone();
    }
    if !continuation {
        if real_user_message(request) && request.subagent_type_name.is_none() {
            if let Some(goal) = request
                .conversation_state
                .as_mut()
                .and_then(|state| state.goal_state.as_mut())
                .filter(|goal| {
                    goal.status == pb::GoalStatus::Paused as i32
                        && owns(goal, conversation_id, request.agent_session_id.as_deref())
                })
            {
                goal.status = pb::GoalStatus::Active as i32;
                goal.idle_continuations_without_tool_calls = 0;
                accrue(goal, now_ms(), true);
            }
        }
        return Ok(None);
    }
    let mut state = request.conversation_state.clone().unwrap_or_default();
    crate::cursor::checkpoint::subagents::refresh(store, checkpoint_id, &mut state).await?;
    if let Some(latest) = stored.as_mut() {
        crate::cursor::checkpoint::subagents::refresh(store, checkpoint_id, latest).await?;
    }
    let Some(mut goal) = state.goal_state.clone() else {
        // Cursor's Clear action removes the optional goal field entirely.
        if let Some(mut latest) = stored {
            if latest.goal_state.as_ref().is_some_and(|goal| {
                owns(goal, conversation_id, request.agent_session_id.as_deref())
            }) {
                latest.goal_state = None;
                crate::cursor::checkpoint::subagents::save(store, checkpoint_id, &mut latest)
                    .await?;
            }
            return Ok(Some(latest));
        }
        return Ok(Some(state));
    };
    if request.subagent_type_name.is_some()
        || Uuid::parse_str(&goal.goal_id).is_err()
        || !owns(&goal, conversation_id, request.agent_session_id.as_deref())
    {
        return Ok(Some(state));
    }
    if let Some(latest) = stored.as_ref() {
        let stale = match latest.goal_state.as_ref() {
            None => {
                latest.root_prompt_messages_json.len() >= state.root_prompt_messages_json.len()
                    && latest.self_summary_count >= state.self_summary_count
            }
            Some(previous) => {
                previous.goal_id != goal.goal_id
                    || previous.continuation_count > goal.continuation_count
                    || (goal.status == pb::GoalStatus::Active as i32
                        && match pb::GoalStatus::try_from(previous.status) {
                            Ok(pb::GoalStatus::Paused) => !toolbar_resumed(previous, &goal),
                            Ok(pb::GoalStatus::Active) => false,
                            _ => true,
                        })
            }
        };
        if stale {
            return Ok(stored);
        }
    }
    if goal.status == pb::GoalStatus::Active as i32
        && state
            .pending_tool_calls
            .iter()
            .any(|pending| pending_blocks_continuation(pending))
    {
        return Ok(Some(state));
    }
    // Notifications may carry the checkpoint from before a later user turn or
    // compaction. Continue from its proven descendant, never rewind that work.
    if let Some(latest) = stored.as_ref() {
        if extends_history(store, latest, &state).await? {
            if let Some(current) = latest.goal_state.as_ref() {
                let incoming = goal;
                goal = current.clone();
                // Counters belong to the latest committed work. The client can
                // still pause/resume, or edit the objective on this checkpoint.
                goal.status = incoming.status;
                if state.root_prompt_messages_json == latest.root_prompt_messages_json {
                    goal.objective = incoming.objective;
                }
                let resumed_clock = incoming.last_accrued_at_ms > current.last_accrued_at_ms
                    && incoming.active_duration_ms >= current.active_duration_ms;
                if incoming.status != current.status || resumed_clock {
                    goal.active_duration_ms = incoming.active_duration_ms;
                    goal.last_accrued_at_ms = incoming.last_accrued_at_ms;
                    if incoming.status == pb::GoalStatus::Active as i32 {
                        goal.idle_continuations_without_tool_calls = 0;
                    }
                }
            }
            state = latest.clone();
        }
    }
    if goal.status != pb::GoalStatus::Active as i32
        || goal.idle_continuations_without_tool_calls >= 3
    {
        if goal.status == pb::GoalStatus::Active as i32 {
            goal.status = pb::GoalStatus::Paused as i32;
        }
        accrue(&mut goal, now_ms(), false);
        state.goal_state = Some(goal);
        crate::cursor::checkpoint::subagents::save(store, checkpoint_id, &mut state).await?;
        return Ok(Some(state));
    }
    goal.continuation_count = goal.continuation_count.saturating_add(1);
    goal.idle_continuations_without_tool_calls += 1;
    accrue(&mut goal, now_ms(), true);
    // A user turn can win activation after preparation. Include the input history
    // so a later wakeup cannot reuse that abandoned turn's old checkpoint anchor.
    let history_id =
        BlobId::digest(&serde_json::to_vec(&state.root_prompt_messages_json)?).to_base64();
    let user = pb::UserMessage {
        message_id: format!("goal:{}:{}:{}", goal.goal_id, goal.continuation_count, history_id),
        text: format!("Continue working on the active goal: {}\nPreserve the full requested scope and verify the result. Use UpdateGoal with status complete only when the objective is achieved and no required work remains. Do not create another goal for this continuation. If blocked, explain the concrete missing input rather than claiming completion.", goal.objective),
        mode: state.mode.unwrap_or(pb::AgentMode::Agent as i32),
        is_simulated_msg: Some(true), simulated_msg_reason: Some(pb::SimulatedMsgReason::GoalContinuation as i32),
        ..Default::default()
    };
    request.action.as_mut().expect("continuation action").action = Some(
        pb::conversation_action::Action::UserMessageAction(pb::UserMessageAction {
            user_message: Some(user),
            ..Default::default()
        }),
    );
    state.goal_state = Some(goal);
    request.conversation_state = Some(state);
    Ok(None)
}

fn real_user_message(request: &pb::AgentRunRequest) -> bool {
    let Some(pb::conversation_action::Action::UserMessageAction(action)) = request
        .action
        .as_ref()
        .and_then(|action| action.action.as_ref())
    else {
        return false;
    };
    action
        .user_message
        .as_ref()
        .is_some_and(|user| user.is_simulated_msg != Some(true) && user.text.trim() != "/summarize")
}

fn toolbar_resumed(previous: &pb::GoalState, incoming: &pb::GoalState) -> bool {
    incoming.idle_continuations_without_tool_calls == 0
        && incoming.active_duration_ms.unwrap_or(0) >= previous.active_duration_ms.unwrap_or(0)
        && incoming.last_accrued_at_ms.is_some()
}

async fn extends_history(
    store: &Store,
    latest: &pb::ConversationStateStructure,
    incoming: &pb::ConversationStateStructure,
) -> Result<bool> {
    let roots = incoming
        .root_prompt_messages_json
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>();
    if latest
        .root_prompt_messages_json
        .iter()
        .skip(1)
        .cloned()
        .collect::<Vec<_>>()
        .starts_with(&roots)
    {
        return Ok(true);
    }
    for archive_id in &latest.summary_archives {
        if let Some(bytes) = store.get_blob(&BlobId::from_bytes(archive_id)?).await? {
            let archive = pb::ConversationSummaryArchive::decode(bytes.as_slice())?;
            if archive.summarized_messages.starts_with(&roots) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn pending_blocks_continuation(pending: &str) -> bool {
    let Ok(message) = serde_json::from_str::<serde_json::Value>(pending) else {
        return true;
    };
    let Some(parts) = message.get("content").and_then(serde_json::Value::as_array) else {
        return true;
    };
    match message.get("role").and_then(serde_json::Value::as_str) {
        Some("assistant") => parts.iter().any(|part| {
            !matches!(
                part.get("type").and_then(serde_json::Value::as_str),
                Some("text" | "reasoning")
            )
        }),
        Some("tool") => parts.iter().any(|part| {
            part.get("type").and_then(serde_json::Value::as_str) != Some("tool-result")
        }),
        _ => true,
    }
}
