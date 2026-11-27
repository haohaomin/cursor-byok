//! Verifies persisted settings are normalized at both storage boundaries.
#[path = "support/fixtures.rs"]
mod fixtures;

use cursor_server::store::{CompactionSettings, Store, DEFAULT_AUTO_COMPACTION_TIMEOUT_SECS};

#[tokio::test]
async fn compaction_settings_default_clamp_and_round_trip() {
    let (_directory, store): (_, Store) = fixtures::temp_store().await;

    assert_eq!(
        store.compaction_settings().await.unwrap(),
        CompactionSettings {
            auto_compaction_timeout_secs: DEFAULT_AUTO_COMPACTION_TIMEOUT_SECS,
        }
    );

    let minimum = store
        .set_compaction_settings(CompactionSettings {
            auto_compaction_timeout_secs: 1,
        })
        .await
        .unwrap();
    assert_eq!(minimum.auto_compaction_timeout_secs, 30);

    let maximum = store
        .set_compaction_settings(CompactionSettings {
            auto_compaction_timeout_secs: 9_999,
        })
        .await
        .unwrap();
    assert_eq!(maximum.auto_compaction_timeout_secs, 3_600);
    assert_eq!(store.compaction_settings().await.unwrap(), maximum);

    sqlx::query(
        "INSERT INTO service_settings(setting_key, value_json, updated_at_ms) VALUES (?, ?, ?) ON CONFLICT(setting_key) DO UPDATE SET value_json = excluded.value_json, updated_at_ms = excluded.updated_at_ms",
    )
    .bind("run_compaction")
    .bind(r#"{"auto_compaction_timeout_secs":0}"#)
    .bind(0_i64)
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(
        store
            .compaction_settings()
            .await
            .unwrap()
            .auto_compaction_timeout_secs,
        30
    );

    sqlx::query("UPDATE service_settings SET value_json = ? WHERE setting_key = ?")
        .bind(r#"{"auto_compaction_timeout_secs":7201}"#)
        .bind("run_compaction")
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(
        store
            .compaction_settings()
            .await
            .unwrap()
            .auto_compaction_timeout_secs,
        3_600
    );
}
