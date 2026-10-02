//! Provides the request-scoped input, subscription, and terminal interface.

use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    cursor::{
        conversation::TransportCommand,
        protocol::{connect, proto::agent::v1 as pb},
        services::observability::CursorTraceRecorder,
    },
    Error, Result,
};

use super::{OutputHub, TransportAdmission, TransportLifecycle};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransportParent {
    pub request_id: Option<String>,
    pub tool_call_id: Option<String>,
}

#[derive(Clone)]
pub struct TransportHandle {
    request_id: String,
    commands: mpsc::Sender<TransportCommand>,
    output: Arc<OutputHub>,
    conversation_id: Arc<OnceLock<String>>,
    parent: Arc<Mutex<Option<TransportParent>>>,
    trace: CursorTraceRecorder,
    lifecycle: TransportLifecycle,
    disconnect: CancellationToken,
}

impl TransportHandle {
    pub(crate) fn new(
        request_id: String,
        commands: mpsc::Sender<TransportCommand>,
        output: Arc<OutputHub>,
        trace: CursorTraceRecorder,
    ) -> Self {
        Self {
            request_id,
            commands,
            output,
            conversation_id: Arc::new(OnceLock::new()),
            parent: Arc::new(Mutex::new(None)),
            trace,
            lifecycle: TransportLifecycle::new(),
            disconnect: CancellationToken::new(),
        }
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn set_conversation_id(&self, conversation_id: &str) -> Result<()> {
        if conversation_id.is_empty() {
            return Err(Error::Protocol("Cursor conversation id is required".into()));
        }
        if self
            .conversation_id
            .get()
            .is_some_and(|current| current != conversation_id)
        {
            return Err(Error::Protocol(format!(
                "conflicting conversation ids for request {}",
                self.request_id
            )));
        }
        let _ = self.conversation_id.set(conversation_id.into());
        Ok(())
    }

    pub fn conversation_id(&self) -> Option<&str> {
        self.conversation_id.get().map(String::as_str)
    }

    pub fn set_parent(&self, parent: TransportParent) -> Result<()> {
        if (parent.request_id.is_none() && parent.tool_call_id.is_none())
            || parent.request_id.as_ref().is_some_and(String::is_empty)
            || parent.tool_call_id.as_ref().is_some_and(String::is_empty)
        {
            return Err(Error::Protocol(
                "Cursor parent ids must not be empty".into(),
            ));
        }
        let mut stored = self.parent.lock().expect("parent metadata lock");
        if let Some(current) = stored.as_mut() {
            // Headers can arrive independently; enrich missing fields without
            // allowing an existing association to be changed or cleared.
            if current
                .request_id
                .as_ref()
                .zip(parent.request_id.as_ref())
                .is_some_and(|(a, b)| a != b)
                || current
                    .tool_call_id
                    .as_ref()
                    .zip(parent.tool_call_id.as_ref())
                    .is_some_and(|(a, b)| a != b)
            {
                return Err(Error::Protocol(format!(
                    "conflicting parent ids for request {}",
                    self.request_id
                )));
            }
            current.request_id = current.request_id.take().or(parent.request_id);
            current.tool_call_id = current.tool_call_id.take().or(parent.tool_call_id);
        } else {
            *stored = Some(parent);
        }
        Ok(())
    }

    pub fn parent(&self) -> Option<TransportParent> {
        self.parent.lock().expect("parent metadata lock").clone()
    }

    pub async fn command(&self, command: TransportCommand) -> Result<()> {
        self.commands
            .send(command)
            .await
            .map_err(|_| Error::RunNotFound(self.request_id.clone()))
    }

    pub async fn disconnect(&self) {
        let _ = self.commands.send(TransportCommand::Disconnect).await;
    }

    pub fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<Bytes> {
        self.output.subscribe()
    }

    pub fn emit_frame(&self, frame: Bytes) -> bool {
        self.output.emit(frame)
    }

    pub fn emit(&self, message: &pb::AgentServerMessage) -> Result<()> {
        if self.emit_frame(connect::encode_message(message)?) {
            Ok(())
        } else {
            Err(Error::RunNotFound(self.request_id.clone()))
        }
    }

    pub(crate) fn close_output(&self) -> bool {
        self.output.close()
    }

    pub(crate) fn trace(&self) -> Option<&CursorTraceRecorder> {
        Some(&self.trace)
    }

    pub(crate) fn accepting_appends(&self) -> bool {
        self.lifecycle.is_open()
    }

    pub(crate) fn admit(&self) -> Result<TransportAdmission> {
        self.lifecycle
            .admit()
            .ok_or_else(|| Error::RunNotFound(self.request_id.clone()))
    }

    pub(crate) fn begin_close(&self) {
        self.lifecycle.begin_close();
    }

    pub(crate) fn admissions_drained(&self) -> bool {
        self.lifecycle.admissions_drained()
    }

    pub(crate) async fn wait_admissions_drained(&self) {
        self.lifecycle.wait_admissions_drained().await;
    }

    pub(crate) fn mark_draining(&self) {
        self.lifecycle.mark_draining();
    }

    pub(crate) fn reopen(&self) {
        self.lifecycle.reopen();
    }

    pub(crate) fn close_transport(&self) {
        self.lifecycle.close();
    }

    pub(crate) async fn wait_transport_closed(&self) {
        self.lifecycle.wait_closed().await;
    }

    pub(crate) fn disconnect_token(&self) -> CancellationToken {
        self.disconnect.clone()
    }

    pub(crate) fn mark_disconnected(&self) {
        self.disconnect.cancel();
    }
}
