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
            ..Default::default()
        }
    );

    let minimum = store
        .set_compaction_settings(cursor_server::store::CompactionSettingsUpdate {
            auto_compaction_timeout_secs: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(minimum.auto_compaction_timeout_secs, 30);

    let maximum = store
        .set_compaction_settings(cursor_server::store::CompactionSettingsUpdate {
            auto_compaction_timeout_secs: Some(9_999),
            ..Default::default()
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

#[tokio::test]
async fn compaction_output_is_independent_normalized_and_atomic() {
    use cursor_server::store::CompactionSettingsUpdate;
    let (_directory, store) = fixtures::temp_store().await;
    store
        .set_compaction_settings(CompactionSettingsUpdate {
            auto_compaction_timeout_secs: Some(600),
            ..Default::default()
        })
        .await
        .unwrap();
    for (requested, expected) in [(1, 1024), (999_999, 65_536), (24_000, 24_000)] {
        let saved = store
            .set_compaction_settings(CompactionSettingsUpdate {
                auto_compaction_output_tokens: Some(requested),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(saved.auto_compaction_timeout_secs, 600);
        assert_eq!(saved.auto_compaction_output_tokens, expected);
        assert_eq!(store.compaction_settings().await.unwrap(), saved);
    }
    let (first, second) = tokio::join!(
        store.set_compaction_settings(CompactionSettingsUpdate {
            auto_compaction_timeout_secs: Some(900),
            ..Default::default()
        }),
        store.set_compaction_settings(CompactionSettingsUpdate {
            auto_compaction_output_tokens: Some(32_768),
            ..Default::default()
        })
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(
        store.compaction_settings().await.unwrap(),
        CompactionSettings {
            auto_compaction_timeout_secs: 900,
            auto_compaction_output_tokens: 32_768
        }
    );
    // A setting row only owns its own value, including when edited outside the UI.
    sqlx::query("UPDATE service_settings SET value_json = '0' WHERE setting_key = 'run_compaction_output_tokens'").execute(store.pool()).await.unwrap();
    assert_eq!(
        store
            .compaction_settings()
            .await
            .unwrap()
            .auto_compaction_output_tokens,
        1024
    );
    sqlx::query("UPDATE service_settings SET value_json = '999999' WHERE setting_key = 'run_compaction_output_tokens'").execute(store.pool()).await.unwrap();
    assert_eq!(
        store
            .compaction_settings()
            .await
            .unwrap()
            .auto_compaction_output_tokens,
        65_536
    );
    // If either write fails, the pair is rolled back, including the timeout row.
    sqlx::query("CREATE TRIGGER reject_output BEFORE UPDATE ON service_settings WHEN NEW.setting_key = 'run_compaction_output_tokens' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END").execute(store.pool()).await.unwrap();
    assert!(store
        .set_compaction_settings(CompactionSettingsUpdate {
            auto_compaction_timeout_secs: Some(1200),
            auto_compaction_output_tokens: Some(8192)
        })
        .await
        .is_err());
    assert_eq!(
        store
            .compaction_settings()
            .await
            .unwrap()
            .auto_compaction_timeout_secs,
        900
    );
}
