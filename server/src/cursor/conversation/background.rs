//! Admits each background completion once before importing state or waking a Run.
use std::{collections::HashSet, sync::Arc};

use parking_lot::Mutex;

use crate::{
    cursor::{compile, protocol::proto::agent::v1 as pb},
    model::ConversationId,
    store::Store,
    Result,
};

type Key = (ConversationId, String);

#[derive(Clone, Default)]
pub(super) struct BackgroundCompletions(Arc<Mutex<HashSet<Key>>>);

pub(super) enum Admission {
    Ordinary,
    New(CompletionGuard),
    Duplicate,
}

pub(super) struct CompletionGuard {
    pending: BackgroundCompletions,
    key: Key,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        self.pending.0.lock().remove(&self.key);
    }
}

impl BackgroundCompletions {
    pub async fn admit(
        &self,
        store: &Store,
        request_id: &str,
        request: &pb::AgentRunRequest,
    ) -> Result<Admission> {
        let Some(event_id) = compile::background_completion_event_id(request)? else {
            return Ok(Admission::Ordinary);
        };
        let conversation = ConversationId::new(
            request
                .conversation_id
                .clone()
                .unwrap_or_else(|| request_id.into()),
        );
        let key = (conversation, event_id);
        if !self.0.lock().insert(key.clone()) {
            return Ok(Admission::Duplicate);
        }
        let guard = CompletionGuard {
            pending: self.clone(),
            key,
        };
        // Canonical event identity survives transport replacement, restart and
        // compaction. Once delivered, automatic redelivery must never become a
        // generic Resume, even if the consuming Run was later cancelled.
        if store
            .message(&guard.key.0, &format!("runtime:{}", guard.key.1))
            .await?
            .is_some()
        {
            return Ok(Admission::Duplicate);
        }
        // Keep concurrent copies out until delivery has committed or the Run
        // ends. A preparation failure before persistence releases the claim.
        Ok(Admission::New(guard))
    }
}
