//! Maps conversation IDs to active conversation runtimes.

use std::{collections::HashMap, sync::Arc};

use tokio::sync::{mpsc, Mutex, Notify};

use crate::{
    cursor::{prompting::PromptCompiler, transport::TransportHandle},
    model::{ConversationId, RunId},
    provider::Provider,
    run::{CommandResult, RunHandle},
    search::WebCache,
    store::Store,
};

use super::{CompiledMessages, MessageDelivery, PendingMessages, TransportCommand};

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;

#[derive(Clone)]
pub struct ConversationRegistry {
    inner: Arc<RegistryInner>,
}

#[derive(Clone)]
pub(crate) struct ConversationDependencies {
    pub store: Store,
    pub provider: Arc<dyn Provider>,
    pub compiler: PromptCompiler,
    pub web_cache: WebCache,
    /// 本地 rules 服务的 md 存储目录;编译请求上下文时合并其中的规则。
    pub local_rules_dir: Option<std::path::PathBuf>,
}

struct RegistryInner {
    background: super::background::BackgroundCompletions,
    goals: crate::cursor::goals::GoalAdmissions,
    current: Mutex<HashMap<ConversationId, ActiveRun>>,
    pending: Mutex<HashMap<ConversationId, PendingMessages>>,
    changed: Notify,
    pub dependencies: ConversationDependencies,
}

#[derive(Clone)]
struct ActiveRun {
    run_id: RunId,
    handle: RunHandle,
}

impl ConversationRegistry {
    pub fn new(
        store: Store,
        provider: Arc<dyn Provider>,
        compiler: PromptCompiler,
        web_cache: WebCache,
        local_rules_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(RegistryInner {
                background: super::background::BackgroundCompletions::default(),
                goals: Default::default(),
                current: Mutex::new(HashMap::new()),
                pending: Mutex::new(HashMap::new()),
                changed: Notify::new(),
                dependencies: ConversationDependencies {
                    store,
                    provider,
                    compiler,
                    web_cache,
                    local_rules_dir,
                },
            }),
        }
    }

    pub(crate) fn dependencies(&self) -> &ConversationDependencies {
        &self.inner.dependencies
    }

    pub(crate) async fn claim_goal(
        &self,
        conversation: &str,
    ) -> Option<crate::cursor::goals::GoalGuard> {
        let guard = self.inner.goals.claim(conversation)?;
        if self
            .inner
            .current
            .lock()
            .await
            .contains_key(&ConversationId::new(conversation))
        {
            return None;
        }
        Some(guard)
    }

    pub(super) async fn admit_background(
        &self,
        request_id: &str,
        request: &mut crate::cursor::protocol::proto::agent::v1::AgentRunRequest,
    ) -> crate::Result<super::background::Admission> {
        crate::cursor::checkpoint::subagents::record(
            &self.inner.dependencies.store,
            request_id,
            request,
        )
        .await?;
        self.inner
            .background
            .admit(&self.inner.dependencies.store, request_id, request)
            .await
    }

    pub(super) async fn publish_idle_checkpoint(
        &self,
        handle: &TransportHandle,
        conversation_id: &str,
    ) -> crate::Result<()> {
        use crate::cursor::protocol::proto::agent::v1 as pb;
        use prost::Message;
        let current = self.inner.current.lock().await;
        let id = ConversationId::new(conversation_id);
        // Hold activation until the synchronous emit completes. A status-only
        // transport must never overwrite a live Run's streaming presentation.
        if current.contains_key(&id) {
            return Ok(());
        }
        let store = &self.inner.dependencies.store;
        if let Some(conversation) = store.conversation(&id).await? {
            if let Some(bytes) = store
                .cursor_checkpoint(conversation.current_checkpoint_id)
                .await?
            {
                handle.emit(&pb::AgentServerMessage {
                    message: Some(
                        pb::agent_server_message::Message::ConversationCheckpointUpdate(
                            pb::ConversationStateStructure::decode(bytes.as_slice())?,
                        ),
                    ),
                    ..Default::default()
                })?;
            }
        }
        Ok(())
    }

    pub(crate) fn bind_transport(
        &self,
        handle: TransportHandle,
        receiver: mpsc::Receiver<TransportCommand>,
    ) {
        super::ConversationRuntime::spawn(self.clone(), handle, receiver);
    }

    pub(crate) async fn activate(
        &self,
        conversation_id: ConversationId,
        run_id: RunId,
        handle: RunHandle,
        replace_existing: bool,
    ) -> bool {
        let mut current = self.inner.current.lock().await;
        if !replace_existing && current.contains_key(&conversation_id) {
            return false;
        }
        let previous = current.insert(
            conversation_id,
            ActiveRun {
                run_id: run_id.clone(),
                handle,
            },
        );
        drop(current);
        if let Some(previous) = previous.filter(|previous| previous.run_id != run_id) {
            previous.handle.cancel();
        }
        true
    }

    pub async fn deliver(
        &self,
        conversation_id: &ConversationId,
        compiled: CompiledMessages,
    ) -> CommandResult {
        self.deliver_with_detached_results(conversation_id, compiled, None)
            .await
    }

    pub(crate) async fn deliver_with_detached_results(
        &self,
        conversation_id: &ConversationId,
        compiled: CompiledMessages,
        detached_results: Option<(crate::model::ToolRoundId, Vec<crate::model::ToolResult>)>,
    ) -> CommandResult {
        if compiled.delivery == MessageDelivery::Ignore {
            return CommandResult::Applied;
        }
        let active = self
            .inner
            .current
            .lock()
            .await
            .get(conversation_id)
            .cloned();
        let Some(active) = active else {
            self.inner
                .pending
                .lock()
                .await
                .entry(conversation_id.clone())
                .or_default()
                .push(compiled);
            return CommandResult::RunEnded;
        };
        if compiled
            .target_run_id
            .as_ref()
            .is_some_and(|target| target != &active.run_id)
        {
            return CommandResult::StaleTarget;
        }
        let pending = compiled.clone();
        let result = match compiled.delivery {
            MessageDelivery::Ignore => CommandResult::Applied,
            MessageDelivery::InsertMessages => {
                active
                    .handle
                    .insert_messages(compiled.event_id, compiled.messages)
                    .await
            }
            MessageDelivery::BreakMessages => {
                active
                    .handle
                    .break_messages_with_detached_results(
                        compiled.event_id,
                        compiled.messages,
                        detached_results,
                    )
                    .await
            }
        };
        if matches!(result, CommandResult::RunClosing | CommandResult::RunEnded) {
            self.inner
                .pending
                .lock()
                .await
                .entry(conversation_id.clone())
                .or_default()
                .push(pending);
        }
        result
    }

    pub(crate) async fn release(
        &self,
        conversation_id: &ConversationId,
        run_id: &RunId,
        handle: &TransportHandle,
        finish: &mut super::RunFinish,
    ) -> crate::Result<()> {
        let mut current = self.inner.current.lock().await;
        if current
            .get(conversation_id)
            .is_some_and(|run| &run.run_id == run_id)
        {
            // A record-only notification can arrive after the final payload was
            // built. Refresh before releasing ownership; a later side transport
            // can only publish once this synchronous emit has completed.
            let refreshed = async {
                let mut interrupted = handle.latest_checkpoint();
                let state = match finish {
                    super::RunFinish::TurnCompleted(state) => Some(state.as_mut()),
                    super::RunFinish::Transport(_) => interrupted.as_mut(),
                };
                if let Some(state) = state {
                    let store = &self.inner.dependencies.store;
                    if let Some(conversation) = store.conversation(conversation_id).await? {
                        let before = state.subagent_runs_by_parent_tool_call_id.clone();
                        crate::cursor::checkpoint::subagents::refresh(
                            store,
                            conversation.current_checkpoint_id,
                            state,
                        )
                        .await?;
                        if before != state.subagent_runs_by_parent_tool_call_id {
                            use crate::cursor::protocol::proto::agent::v1 as pb;
                            handle.emit(&pb::AgentServerMessage {
                                message: Some(
                                    pb::agent_server_message::Message::ConversationCheckpointUpdate(
                                        state.clone(),
                                    ),
                                ),
                                ..Default::default()
                            })?;
                        }
                    }
                }
                Ok(())
            }
            .await;
            current.remove(conversation_id);
            self.inner.changed.notify_waiters();
            return refreshed;
        }
        Ok(())
    }

    pub(crate) async fn wait_until_idle(&self, conversation_id: &ConversationId) {
        loop {
            let changed = self.inner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self
                .inner
                .current
                .lock()
                .await
                .contains_key(conversation_id)
            {
                return;
            }
            changed.await;
        }
    }

    pub(crate) async fn take_pending(
        &self,
        conversation_id: &ConversationId,
    ) -> Vec<CompiledMessages> {
        self.inner
            .pending
            .lock()
            .await
            .remove(conversation_id)
            .map(|mut pending| pending.drain().collect())
            .unwrap_or_default()
    }

    pub async fn shutdown(&self) {
        let current = std::mem::take(&mut *self.inner.current.lock().await);
        for active in current.into_values() {
            active.handle.cancel();
        }
    }
}
