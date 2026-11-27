//! Persist independently editable compaction limits in one atomic settings snapshot.
use super::{now_ms, Store};
use crate::Result;
use serde::{Deserialize, Serialize};

const TIMEOUT_KEY: &str = "run_compaction";
const OUTPUT_KEY: &str = "run_compaction_output_tokens";
pub const DEFAULT_AUTO_COMPACTION_TIMEOUT_SECS: u64 = 180;
pub const DEFAULT_AUTO_COMPACTION_OUTPUT_TOKENS: u64 = 16_384;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CompactionSettings {
    pub auto_compaction_timeout_secs: u64,
    pub auto_compaction_output_tokens: u64,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            auto_compaction_timeout_secs: DEFAULT_AUTO_COMPACTION_TIMEOUT_SECS,
            auto_compaction_output_tokens: DEFAULT_AUTO_COMPACTION_OUTPUT_TOKENS,
        }
    }
}

/// Only edited fields are sent. Missing fields stay unchanged; null is invalid.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionSettingsUpdate {
    #[serde(default, deserialize_with = "present_u64")]
    pub auto_compaction_timeout_secs: Option<u64>,
    #[serde(default, deserialize_with = "present_u64")]
    pub auto_compaction_output_tokens: Option<u64>,
}

fn present_u64<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

/// The timeout row owns only the timeout. Output tokens have their own key;
/// changing one setting never rewrites or backfills the other setting's row.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TimeoutSettings {
    auto_compaction_timeout_secs: u64,
}

async fn read_settings<'e>(
    executor: impl sqlx::Executor<'e, Database = sqlx::Sqlite>,
) -> Result<CompactionSettings> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT setting_key, value_json FROM service_settings WHERE setting_key IN (?, ?)",
    )
    .bind(TIMEOUT_KEY)
    .bind(OUTPUT_KEY)
    .fetch_all(executor)
    .await?;
    let mut settings = CompactionSettings::default();
    for (key, value) in rows {
        if key == TIMEOUT_KEY {
            settings.auto_compaction_timeout_secs =
                serde_json::from_str::<TimeoutSettings>(&value)?
                    .auto_compaction_timeout_secs
                    .clamp(30, 3600);
        } else {
            settings.auto_compaction_output_tokens =
                serde_json::from_str::<u64>(&value)?.clamp(1024, 65536);
        }
    }
    Ok(settings)
}

impl Store {
    pub async fn compaction_settings(&self) -> Result<CompactionSettings> {
        read_settings(&self.pool).await
    }

    pub async fn set_compaction_settings(
        &self,
        update: CompactionSettingsUpdate,
    ) -> Result<CompactionSettings> {
        let _write = self.writes.lock().await;
        let mut tx = self.pool.begin().await?;
        let timeout = update
            .auto_compaction_timeout_secs
            .map(|seconds| {
                serde_json::to_string(&TimeoutSettings {
                    auto_compaction_timeout_secs: seconds.clamp(30, 3600),
                })
            })
            .transpose()?;
        let output = update
            .auto_compaction_output_tokens
            .map(|tokens| tokens.clamp(1024, 65536).to_string());
        for (key, value) in [(TIMEOUT_KEY, timeout), (OUTPUT_KEY, output)] {
            if let Some(value) = value {
                sqlx::query("INSERT INTO service_settings(setting_key, value_json, updated_at_ms) VALUES (?, ?, ?) ON CONFLICT(setting_key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms")
                    .bind(key).bind(value).bind(now_ms()).execute(&mut *tx).await?;
            }
        }
        let settings = read_settings(&mut *tx).await?;
        tx.commit().await?;
        Ok(settings)
    }
}
