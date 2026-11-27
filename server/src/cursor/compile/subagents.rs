//! Projects only the running custom subagent's role into append-only context.
use crate::cursor::protocol::proto::agent::v1 as pb;

use super::context::{neutralize_prompt_body, xml};

// State is explicit even when no role is selected: omission cannot revoke
// instructions that remain in append-only history.
const INACTIVE: &str = "<custom_subagent active=\"false\">\nNo custom subagent role is active. Previous custom subagent instructions no longer apply.\n</custom_subagent>";

pub(super) fn instructions(selected: Option<&str>, context: &pb::RequestContext) -> String {
    let Some(agent) = selected.filter(|name| !name.is_empty()).and_then(|name| {
        context
            .custom_subagents
            .iter()
            .find(|agent| agent.name == name)
    }) else {
        return INACTIVE.to_owned();
    };
    if agent.prompt.trim().is_empty() {
        return INACTIVE.to_owned();
    }
    format!(
        "<custom_subagent active=\"true\" name=\"{}\">\nApply the following instructions for your current subagent role, replacing any previous custom subagent instructions.\n{}\n\n{}\n</custom_subagent>",
        xml(&agent.name),
        neutralize_prompt_body(&agent.description),
        neutralize_prompt_body(&agent.prompt),
    )
}
