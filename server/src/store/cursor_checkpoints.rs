//! Stores Cursor presentation state alongside its immutable canonical checkpoint.
use crate::{
    model::{CheckpointId, ConversationId},
    Result,
};

use super::Store;

pub(crate) struct CursorCheckpointUpdate {
    pub state: Vec<u8>,
    pub subagent_runs: Vec<(String, Vec<u8>)>,
}

impl Store {
    /// Read and replace presentation state atomically, including inherited state.
    /// The callback owns decoding/merging; storage stays independent of protobuf.
    pub(crate) async fn update_cursor_checkpoint(
        &self,
        checkpoint_id: CheckpointId,
        update: impl FnOnce(Option<&[u8]>, &[(String, Vec<u8>)]) -> Result<CursorCheckpointUpdate>,
    ) -> Result<Vec<u8>> {
        let _write = self.writes.lock().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let next = update_checkpoint_tx(&mut tx, checkpoint_id, update).await?;
        tx.commit().await?;
        Ok(next)
    }

    /// Select the live conversation head inside the same write transaction.
    pub(crate) async fn update_current_cursor_checkpoint(
        &self,
        conversation_id: &ConversationId,
        update: impl FnOnce(Option<&[u8]>, &[(String, Vec<u8>)]) -> Result<CursorCheckpointUpdate>,
    ) -> Result<Vec<u8>> {
        let _write = self.writes.lock().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let id = Self::ensure_conversation_tx(&mut tx, conversation_id).await?;
        let next = update_checkpoint_tx(&mut tx, id, update).await?;
        tx.commit().await?;
        Ok(next)
    }

    #[cfg(test)]
    pub(crate) async fn save_cursor_checkpoint(
        &self,
        checkpoint_id: CheckpointId,
        state: &[u8],
    ) -> Result<()> {
        self.update_cursor_checkpoint(checkpoint_id, |_, _| {
            Ok(CursorCheckpointUpdate {
                state: state.to_vec(),
                subagent_runs: Vec::new(),
            })
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn cursor_subagent_runs(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(sqlx::query_as(
            "SELECT s.parent_tool_call_id, s.state FROM cursor_subagent_runs s
             JOIN conversation_checkpoints c ON c.conversation_id = s.conversation_id
             WHERE c.checkpoint_id = ?",
        )
        .bind(checkpoint_id.0)
        .fetch_all(&self.pool)
        .await?)
    }

    pub(crate) async fn cursor_checkpoint(
        &self,
        checkpoint_id: CheckpointId,
    ) -> Result<Option<Vec<u8>>> {
        Ok(sqlx::query_scalar(
            "WITH RECURSIVE lineage(checkpoint_id, parent_checkpoint_id, depth) AS (
                 SELECT checkpoint_id, parent_checkpoint_id, 0
                 FROM conversation_checkpoints WHERE checkpoint_id = ?
                 UNION ALL
                 SELECT c.checkpoint_id, c.parent_checkpoint_id, l.depth + 1
                 FROM conversation_checkpoints c JOIN lineage l ON c.checkpoint_id = l.parent_checkpoint_id
             )
             SELECT s.state FROM lineage l JOIN cursor_checkpoints s ON s.checkpoint_id = l.checkpoint_id
             ORDER BY l.depth LIMIT 1",
        )
        .bind(checkpoint_id.0)
        .fetch_optional(&self.pool)
        .await?)
    }
}

async fn update_checkpoint_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    checkpoint: CheckpointId,
    update: impl FnOnce(Option<&[u8]>, &[(String, Vec<u8>)]) -> Result<CursorCheckpointUpdate>,
) -> Result<Vec<u8>> {
    let conversation: String = sqlx::query_scalar(
        "SELECT conversation_id FROM conversation_checkpoints WHERE checkpoint_id = ?",
    )
    .bind(checkpoint.0)
    .fetch_one(&mut **tx)
    .await?;
    let subagents: Vec<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT parent_tool_call_id, state FROM cursor_subagent_runs WHERE conversation_id = ?",
    )
    .bind(&conversation)
    .fetch_all(&mut **tx)
    .await?;
    let previous: Option<Vec<u8>> = sqlx::query_scalar(
        "WITH RECURSIVE lineage(checkpoint_id, parent_checkpoint_id, depth) AS (
             SELECT checkpoint_id, parent_checkpoint_id, 0
             FROM conversation_checkpoints WHERE checkpoint_id = ?
             UNION ALL
             SELECT c.checkpoint_id, c.parent_checkpoint_id, l.depth + 1
             FROM conversation_checkpoints c JOIN lineage l ON c.checkpoint_id = l.parent_checkpoint_id
         )
         SELECT s.state FROM lineage l JOIN cursor_checkpoints s ON s.checkpoint_id = l.checkpoint_id
         ORDER BY l.depth LIMIT 1",
    ).bind(checkpoint.0).fetch_optional(&mut **tx).await?;
    let next = update(previous.as_deref(), &subagents)?;
    sqlx::query(
        "INSERT INTO cursor_checkpoints(checkpoint_id, state) VALUES (?, ?)
                 ON CONFLICT(checkpoint_id) DO UPDATE SET state = excluded.state",
    )
    .bind(checkpoint.0)
    .bind(&next.state)
    .execute(&mut **tx)
    .await?;
    for (id, state) in next.subagent_runs {
        sqlx::query(
            "INSERT INTO cursor_subagent_runs(conversation_id, parent_tool_call_id, state)
             VALUES (?, ?, ?) ON CONFLICT(conversation_id, parent_tool_call_id)
             DO UPDATE SET state = excluded.state",
        )
        .bind(&conversation)
        .bind(id)
        .bind(state)
        .execute(&mut **tx)
        .await?;
    }
    Ok(next.state)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use sqlx::{
        migrate::Migrator,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };

    use super::Store;
    use crate::model::{CanonicalMessage, CheckpointId, ConversationId, Origin, Role};

    #[tokio::test]
    async fn upgrade_preserves_history_and_recovers_checkpoint_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoints.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        let all = sqlx::migrate!("./migrations");
        let prior = Migrator {
            migrations: Cow::Owned(
                all.iter()
                    .filter(|migration| migration.version <= 10)
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        };
        prior.run(&pool).await.unwrap();
        let old = Store {
            pool,
            writes: Default::default(),
        };
        let conversation = ConversationId::new("preserved-conversation");
        let root = old.ensure_conversation(&conversation).await.unwrap();
        let history = vec![CanonicalMessage::text(
            "old-user",
            Role::User,
            Origin::User,
            "Keep this history.",
        )];
        let child = old
            .import_checkpoint(&conversation, &history)
            .await
            .unwrap();
        old.pool.close().await;

        let store = Store::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        assert_eq!(
            store.load_checkpoint_messages(child).await.unwrap(),
            history
        );
        assert_eq!(store.cursor_checkpoint(child).await.unwrap(), None);
        store
            .save_cursor_checkpoint(root, b"root state")
            .await
            .unwrap();
        assert_eq!(
            store.cursor_checkpoint(child).await.unwrap().as_deref(),
            Some(b"root state".as_slice())
        );
        store
            .save_cursor_checkpoint(child, b"child state")
            .await
            .unwrap();
        store
            .save_cursor_checkpoint(child, b"settled child state")
            .await
            .unwrap();
        assert_eq!(
            store.cursor_checkpoint(child).await.unwrap().as_deref(),
            Some(b"settled child state".as_slice())
        );
        assert!(store
            .save_cursor_checkpoint(CheckpointId(i64::MAX), b"invalid")
            .await
            .is_err());
        let other = store
            .ensure_conversation(&ConversationId::new("other-conversation"))
            .await
            .unwrap();
        assert_eq!(store.cursor_checkpoint(other).await.unwrap(), None);
        store.pool.close().await;
        let reopened = Store::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        assert_eq!(
            reopened.cursor_checkpoint(child).await.unwrap().as_deref(),
            Some(b"settled child state".as_slice())
        );
        assert_eq!(
            reopened.load_checkpoint_messages(child).await.unwrap(),
            history
        );
    }

    #[tokio::test]
    async fn subagent_table_upgrade_is_atomic_and_scoped_to_the_conversation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("subagents.db");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true)
                    .foreign_keys(true),
            )
            .await
            .unwrap();
        let all = sqlx::migrate!("./migrations");
        Migrator {
            migrations: Cow::Owned(
                all.iter()
                    .filter(|migration| migration.version <= 12)
                    .cloned()
                    .collect(),
            ),
            ..Migrator::DEFAULT
        }
        .run(&pool)
        .await
        .unwrap();
        let old = Store {
            pool,
            writes: Default::default(),
        };
        let conversation = ConversationId::new("existing");
        let root = old.ensure_conversation(&conversation).await.unwrap();
        sqlx::query("INSERT INTO cursor_checkpoints(checkpoint_id, state) VALUES (?, ?)")
            .bind(root.0)
            .bind(b"existing state".as_slice())
            .execute(old.pool())
            .await
            .unwrap();
        old.pool.close().await;
        let store = Store::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        assert_eq!(
            store.cursor_checkpoint(root).await.unwrap().unwrap(),
            b"existing state"
        );
        assert!(store.cursor_subagent_runs(root).await.unwrap().is_empty());
        // Fail after the checkpoint and first run row have been written.
        let result = store
            .update_cursor_checkpoint(root, |_, _| {
                Ok(super::CursorCheckpointUpdate {
                    state: b"uncommitted".to_vec(),
                    subagent_runs: vec![
                        ("valid".into(), b"valid".to_vec()),
                        ("".into(), b"invalid".to_vec()),
                    ],
                })
            })
            .await;
        assert!(result.is_err());
        assert_eq!(
            store.cursor_checkpoint(root).await.unwrap().unwrap(),
            b"existing state"
        );
        assert!(store.cursor_subagent_runs(root).await.unwrap().is_empty());
        store
            .update_cursor_checkpoint(root, |_, _| {
                Ok(super::CursorCheckpointUpdate {
                    state: b"committed".to_vec(),
                    subagent_runs: vec![("worker".into(), b"finished".to_vec())],
                })
            })
            .await
            .unwrap();
        let other = store
            .ensure_conversation(&ConversationId::new("other"))
            .await
            .unwrap();
        assert!(store.cursor_subagent_runs(other).await.unwrap().is_empty());
        store.pool.close().await;
        let reopened = Store::connect(&format!("sqlite://{}", path.display()))
            .await
            .unwrap();
        assert_eq!(
            reopened.cursor_subagent_runs(root).await.unwrap(),
            vec![("worker".into(), b"finished".to_vec())]
        );
        sqlx::query("DELETE FROM conversation_checkpoints WHERE checkpoint_id = ?")
            .bind(root.0)
            .execute(reopened.pool())
            .await
            .unwrap();
        sqlx::query("DELETE FROM conversations WHERE conversation_id = ?")
            .bind(conversation.as_str())
            .execute(reopened.pool())
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM cursor_subagent_runs")
            .fetch_one(reopened.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
