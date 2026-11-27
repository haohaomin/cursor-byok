//! Projects only the running custom subagent's role into append-only context.
use crate::cursor::protocol::proto::agent::v1 as pb;

use super::context::{neutralize_prompt_body, xml};

pub(super) fn instructions(selected: Option<&str>, context: &pb::RequestContext) -> String {
    let Some(agent) = selected.filter(|name| !name.is_empty()).and_then(|name| {
        context
            .custom_subagents
            .iter()
            .find(|agent| agent.name == name)
    }) else {
        return String::new();
    };
    if agent.prompt.trim().is_empty() {
        return String::new();
    }
    format!(
        "<custom_subagent name=\"{}\">\nApply the following instructions for your current subagent role.\n{}\n\n{}\n</custom_subagent>",
        xml(&agent.name),
        neutralize_prompt_body(&agent.description),
        neutralize_prompt_body(&agent.prompt),
    )
}
