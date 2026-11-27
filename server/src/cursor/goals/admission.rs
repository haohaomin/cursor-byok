//! Reserves one automatic goal Run per conversation until that Run finishes.
use parking_lot::Mutex;
use std::{collections::HashSet, sync::Arc};

#[derive(Clone, Default)]
pub(crate) struct GoalAdmissions(Arc<Mutex<HashSet<String>>>);

pub(crate) struct GoalGuard {
    admissions: GoalAdmissions,
    conversation: String,
}

impl GoalAdmissions {
    pub(crate) fn claim(&self, conversation: &str) -> Option<GoalGuard> {
        self.0
            .lock()
            .insert(conversation.into())
            .then(|| GoalGuard {
                admissions: self.clone(),
                conversation: conversation.into(),
            })
    }
}

impl Drop for GoalGuard {
    fn drop(&mut self) {
        self.admissions.0.lock().remove(&self.conversation);
    }
}
