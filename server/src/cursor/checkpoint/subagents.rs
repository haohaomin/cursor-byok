//! Folds notification status into checkpoints without appending model messages.
use prost::Message;

#[cfg(test)]
#[path = "subagents_tests.rs"]
mod tests;

use crate::{
    cursor::{compile, protocol::proto::agent::v1 as pb, tools::runtime::now_ms},
    model::{CheckpointId, ConversationId},
    store::{CursorCheckpointUpdate, Store},
    Error, Result,
};

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn terminal(status: i32) -> bool {
    matches!(
        pb::SubagentRunStatus::try_from(status),
        Ok(pb::SubagentRunStatus::Success
            | pb::SubagentRunStatus::Error
            | pb::SubagentRunStatus::Aborted)
    )
}

fn key(completion: &pb::BackgroundTaskCompletion) -> Option<String> {
    if completion.kind != pb::BackgroundTaskKind::Subagent as i32
        || matches!(
            pb::BackgroundTaskCompletionReason::try_from(completion.reason),
            Ok(pb::BackgroundTaskCompletionReason::TaskProgress
                | pb::BackgroundTaskCompletionReason::WorkerMessage
                | pb::BackgroundTaskCompletionReason::ProjectSessionMessage)
        )
    {
        return None;
    }
    trimmed(completion.tool_call_id.as_deref())
}

pub(crate) fn has_updates(request: &pb::AgentRunRequest) -> bool {
    match request
        .action
        .as_ref()
        .and_then(|action| action.action.as_ref())
    {
        Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(action)) => action
            .completions
            .iter()
            .any(|completion| key(completion).is_some()),
        _ => false,
    }
}

fn apply(
    state: &mut pb::ConversationStateStructure,
    completion: &pb::BackgroundTaskCompletion,
    now: u64,
) -> Result<()> {
    let Some(key) = key(completion) else {
        return Ok(());
    };
    let reason = pb::BackgroundTaskCompletionReason::try_from(completion.reason).map_err(|_| {
        Error::Protocol(format!(
            "unknown background task completion reason: {}",
            completion.reason
        ))
    })?;
    let status = pb::BackgroundTaskStatus::try_from(completion.status).map_err(|_| {
        Error::Protocol(format!(
            "unknown background task status: {}",
            completion.status
        ))
    })?;
    let existing = state
        .subagent_runs_by_parent_tool_call_id
        .get(&key)
        .cloned()
        .unwrap_or_default();
    if terminal(existing.status)
        && completion
            .completed_at_ms
            .zip(existing.completed_timestamp_ms)
            .is_some_and(|(new, old)| new < old)
    {
        return Ok(());
    }
    let agent_id =
        trimmed(completion.subagent_id.as_deref()).or_else(|| trimmed(Some(&completion.task_id)));
    let metadata = agent_id.as_ref().and_then(|id| {
        state.subagent_states.get(id).or_else(|| {
            state.subagent_states.values().find(|state| {
                state
                    .cloud_subagent
                    .as_ref()
                    .is_some_and(|cloud| &cloud.bc_id == id)
            })
        })
    });
    let next = pb::SubagentRunState {
        parent_tool_call_id: key.clone(),
        subagent_id: agent_id,
        environment: metadata
            .map(|state| state.environment)
            .filter(|environment| {
                *environment != pb::SubagentExecutionEnvironment::Unspecified as i32
            })
            .unwrap_or(existing.environment),
        status: match status {
            pb::BackgroundTaskStatus::Success => pb::SubagentRunStatus::Success as i32,
            pb::BackgroundTaskStatus::Error => pb::SubagentRunStatus::Error as i32,
            pb::BackgroundTaskStatus::Aborted => pb::SubagentRunStatus::Aborted as i32,
            pb::BackgroundTaskStatus::Unspecified => existing.status,
        },
        title: trimmed(Some(&completion.title)).or(existing.title),
        detail: trimmed(completion.detail.as_deref()).or(existing.detail),
        transcript_path: metadata
            .and_then(|state| state.cloud_subagent.as_ref())
            .and_then(|cloud| trimmed(cloud.transcript_path.as_deref()))
            .or(existing.transcript_path),
        output_path: trimmed(completion.output_path.as_deref()).or(existing.output_path),
        completed_timestamp_ms: completion
            .completed_at_ms
            .or(existing.completed_timestamp_ms)
            .or(Some(now)),
        completion_reason: if reason == pb::BackgroundTaskCompletionReason::Unspecified {
            existing.completion_reason
        } else {
            Some(reason as i32)
        },
    };
    state.subagent_runs_by_parent_tool_call_id.insert(key, next);
    Ok(())
}

/// A parent projection must not undo a newer notification. Terminal results
/// always beat running state; newer attention/reparent metadata also survives.
pub(crate) fn preserve_recorded(
    state: &mut pb::ConversationStateStructure,
    persisted: &pb::ConversationStateStructure,
) {
    for (id, previous) in &persisted.subagent_runs_by_parent_tool_call_id {
        if !terminal(previous.status) && previous.completed_timestamp_ms.is_none() {
            continue;
        }
        let keep = state
            .subagent_runs_by_parent_tool_call_id
            .get(id)
            .is_none_or(|next| {
                if terminal(previous.status) {
                    !terminal(next.status)
                        || next.completed_timestamp_ms.unwrap_or(0)
                            < previous.completed_timestamp_ms.unwrap_or(0)
                } else {
                    !terminal(next.status)
                        && next.completed_timestamp_ms.unwrap_or(0)
                            <= previous.completed_timestamp_ms.unwrap_or(0)
                }
            });
        if keep {
            state
                .subagent_runs_by_parent_tool_call_id
                .insert(id.clone(), previous.clone());
        }
    }
}

fn recorded_state(runs: &[(String, Vec<u8>)]) -> Result<pb::ConversationStateStructure> {
    Ok(pb::ConversationStateStructure {
        subagent_runs_by_parent_tool_call_id: runs
            .iter()
            .map(|(id, bytes)| Ok((id.clone(), pb::SubagentRunState::decode(bytes.as_slice())?)))
            .collect::<Result<_>>()?,
        ..Default::default()
    })
}

fn update(state: &pb::ConversationStateStructure) -> CursorCheckpointUpdate {
    CursorCheckpointUpdate {
        state: state.encode_to_vec(),
        subagent_runs: state
            .subagent_runs_by_parent_tool_call_id
            .iter()
            .map(|(id, run)| (id.clone(), run.encode_to_vec()))
            .collect(),
    }
}

pub(crate) async fn record(
    store: &Store,
    request_id: &str,
    request: &mut pb::AgentRunRequest,
) -> Result<()> {
    if !has_updates(request) {
        return Ok(());
    }
    // Reject malformed visible notifications before mutating presentation state.
    compile::background_completion_events(request)?;
    let Some(pb::conversation_action::Action::BackgroundTaskCompletionAction(action)) = request
        .action
        .as_ref()
        .and_then(|action| action.action.as_ref())
    else {
        unreachable!()
    };
    let conversation =
        ConversationId::new(request.conversation_id.as_deref().unwrap_or(request_id));
    let bytes = store
        .update_current_cursor_checkpoint(&conversation, |stored, runs| {
            let mut state = match stored {
                Some(bytes) => pb::ConversationStateStructure::decode(bytes)?,
                None => request.conversation_state.clone().unwrap_or_default(),
            };
            if let Some(incoming) = request.conversation_state.as_ref() {
                // The task may have started upstream since our last local turn.
                // Seed absent metadata without replacing locally recorded results.
                for (id, run) in &incoming.subagent_runs_by_parent_tool_call_id {
                    state
                        .subagent_runs_by_parent_tool_call_id
                        .entry(id.clone())
                        .or_insert_with(|| run.clone());
                }
                for (id, metadata) in &incoming.subagent_states {
                    state
                        .subagent_states
                        .entry(id.clone())
                        .or_insert_with(|| metadata.clone());
                }
            }
            let recorded = recorded_state(runs)?;
            for (id, run) in &recorded.subagent_runs_by_parent_tool_call_id {
                state
                    .subagent_runs_by_parent_tool_call_id
                    .entry(id.clone())
                    .or_insert_with(|| run.clone());
            }
            preserve_recorded(&mut state, &recorded);
            let now = now_ms();
            for completion in &action.completions {
                apply(&mut state, completion, now)?;
            }
            Ok(update(&state))
        })
        .await?;
    let recorded = pb::ConversationStateStructure::decode(bytes.as_slice())?;
    if let Some(incoming) = request.conversation_state.as_mut() {
        // Preserve the request's own history; only the status map is updated.
        incoming.subagent_runs_by_parent_tool_call_id =
            recorded.subagent_runs_by_parent_tool_call_id;
    }
    Ok(())
}

pub(crate) async fn refresh(
    store: &Store,
    checkpoint: CheckpointId,
    state: &mut pb::ConversationStateStructure,
) -> Result<()> {
    if let Some(bytes) = store.cursor_checkpoint(checkpoint).await? {
        preserve_recorded(
            state,
            &pb::ConversationStateStructure::decode(bytes.as_slice())?,
        );
    }
    preserve_recorded(
        state,
        &recorded_state(&store.cursor_subagent_runs(checkpoint).await?)?,
    );
    Ok(())
}

pub(crate) async fn save(
    store: &Store,
    checkpoint: CheckpointId,
    state: &mut pb::ConversationStateStructure,
) -> Result<()> {
    let bytes = store
        .update_cursor_checkpoint(checkpoint, |stored, runs| {
            if let Some(bytes) = stored {
                preserve_recorded(state, &pb::ConversationStateStructure::decode(bytes)?);
            }
            preserve_recorded(state, &recorded_state(runs)?);
            Ok(update(state))
        })
        .await?;
    *state = pb::ConversationStateStructure::decode(bytes.as_slice())?;
    Ok(())
}
