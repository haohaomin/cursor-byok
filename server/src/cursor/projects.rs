//! Restores project-root identity independently from the conversation's mode.
use prost::Message;

use crate::{
    model::ConversationId,
    store::{BlobId, Store},
    Error, Result,
};

use super::{protocol::proto::agent::v1 as pb, services::blob_sync::BlobSynchronizer};

pub(crate) const MESSAGE_INSTRUCTIONS: &str = "<project_user_messages>\nThis is the project's root conversation. Use SendMessage for acknowledgements, progress updates and final responses that the user should read. Ordinary assistant text is internal activity in the project's interface. Do not repeat a sent message as ordinary assistant text. After the final SendMessage, end the turn without further ordinary text. Use AskQuestion when you need the user's answer.\n</project_user_messages>";

pub(crate) async fn is_root(
    request: &pb::AgentRunRequest,
    conversation: &ConversationId,
    store: &Store,
    blobs: &BlobSynchronizer,
) -> Result<bool> {
    if request.subagent_type_name.is_some() {
        return Ok(false);
    }
    if let Some(pb::conversation_action::Action::UserMessageAction(action)) = request
        .action
        .as_ref()
        .and_then(|action| action.action.as_ref())
    {
        if action.user_message.as_ref().is_some_and(root_boundary) {
            return Ok(true);
        }
    }
    let saved = match store.conversation(conversation).await? {
        Some(conversation) => store
            .cursor_checkpoint(conversation.current_checkpoint_id)
            .await?
            .map(|bytes| pb::ConversationStateStructure::decode(bytes.as_slice()))
            .transpose()?,
        None => None,
    };
    // A root project cannot be reparented into a project thread. Preserve its
    // recorded identity when a later message or older checkpoint omits it.
    if saved
        .as_ref()
        .into_iter()
        .chain(request.conversation_state.as_ref())
        .any(|state| state.is_root_project_conversation == Some(true))
    {
        return Ok(true);
    }
    let Some(state) = request.conversation_state.as_ref().or(saved.as_ref()) else {
        return Ok(false);
    };
    for raw in state.turns.iter().rev() {
        let id = BlobId::from_bytes(raw)?;
        let bytes = blobs
            .get(&id)
            .await?
            .ok_or_else(|| Error::Protocol("Project history turn is missing".into()))?;
        let turn = pb::ConversationTurnStructure::decode(bytes.as_slice())?;
        let Some(pb::conversation_turn_structure::Turn::AgentConversationTurn(turn)) = turn.turn
        else {
            continue;
        };
        let id = BlobId::from_bytes(&turn.user_message)?;
        let bytes = blobs
            .get(&id)
            .await?
            .ok_or_else(|| Error::Protocol("Project history user message is missing".into()))?;
        let user = pb::UserMessage::decode(bytes.as_slice())?;
        if user.project_details.is_some() || user.best_of_n_group_id.is_some() {
            return Ok(root_boundary(&user));
        }
        if user.is_simulated_msg != Some(true) {
            break;
        }
    }
    Ok(false)
}

fn root_boundary(user: &pb::UserMessage) -> bool {
    user.best_of_n_group_id.is_none()
        && user
            .project_details
            .as_ref()
            .is_some_and(|details| details.subagent.is_none() && details.side_chat.is_none())
}
