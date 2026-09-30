//! Defers narration until tool cards settle, for both live output and checkpoint replay.
use std::time::Duration;

use crate::{
    cursor::{
        checkpoint::StepBuffer,
        protocol::{events, proto::agent::v1 as pb},
        transport::TransportHandle,
    },
    Result,
};

pub(super) enum Narration {
    Text(String),
    TextEnd,
    Thinking(String),
    ThinkingEnd(Duration),
}

#[derive(Default)]
pub(super) struct NarrationBuffer {
    blocked: bool,
    pending: Vec<Narration>,
}

impl NarrationBuffer {
    pub fn tools_started(&mut self, steps: &mut StepBuffer) {
        if !self.blocked {
            // Close any pre-tool segment before recording the completed tool cards.
            steps.finish_model_attempt();
            self.blocked = true;
        }
    }

    pub fn push(
        &mut self,
        event: Narration,
        handle: &TransportHandle,
        steps: &mut StepBuffer,
    ) -> Result<()> {
        if self.blocked {
            self.pending.push(event);
            Ok(())
        } else {
            event.emit(handle, steps)
        }
    }

    pub fn tools_finished(
        &mut self,
        handle: &TransportHandle,
        steps: &mut StepBuffer,
    ) -> Result<()> {
        self.blocked = false;
        for event in self.pending.drain(..) {
            event.emit(handle, steps)?;
        }
        Ok(())
    }

    pub fn discard(&mut self) {
        self.pending.clear();
        self.blocked = false;
    }
}

impl Narration {
    fn emit(self, handle: &TransportHandle, steps: &mut StepBuffer) -> Result<()> {
        let message = match self {
            Self::Text(text) => {
                steps.text_delta(&text);
                pb::interaction_update::Message::TextDelta(pb::TextDeltaUpdate {
                    text,
                    is_server_notice: false,
                })
            }
            Self::TextEnd => {
                steps.finish_text();
                return Ok(());
            }
            Self::Thinking(text) => {
                steps.thinking_delta(&text);
                pb::interaction_update::Message::ThinkingDelta(pb::ThinkingDeltaUpdate {
                    text,
                    thinking_style: Some(pb::ThinkingStyle::Default as i32),
                })
            }
            Self::ThinkingEnd(duration) => {
                steps.finish_thinking(duration);
                return handle.emit(&events::thinking_completed(duration));
            }
        };
        handle.emit(&pb::AgentServerMessage {
            message: Some(pb::agent_server_message::Message::InteractionUpdate(
                pb::InteractionUpdate {
                    message: Some(message),
                },
            )),
            ttft_breakdown: None,
        })
    }
}
