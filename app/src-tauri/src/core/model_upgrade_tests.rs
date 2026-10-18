//! Isolated SQLite fixtures only; no desktop profile, credentials, audio, or cloud calls.
use super::*;
use rusqlite::{params, Connection};
use std::{fs, path::PathBuf};
use uuid::Uuid;

const UPGRADE: &str = "rewrite-model-0.2.11";

struct UpgradeDb(PathBuf);
impl UpgradeDb {
    fn new(config: &ProviderConfig) -> Self {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let base = if manifest.ends_with("harness") { manifest.join("test-data") }
            else { manifest.join("src/core/harness/test-data") };
        fs::create_dir_all(&base).unwrap();
        let file = Self(base.join(format!("model-upgrade-{}.sqlite", Uuid::new_v4())));
        let store = Store::open(&file.0).unwrap();
        store.test_connection().execute("UPDATE provider_config SET version=?1,data=?2 WHERE singleton=1",
            params![config.version, serde_json::to_string(config).unwrap()]).unwrap();
        file
    }
    fn open(&self) -> Store { Store::open(&self.0).unwrap() }
}
impl Drop for UpgradeDb {
    fn drop(&mut self) {
        for path in [self.0.clone(), self.0.with_extension("sqlite-journal"),
            self.0.with_extension("sqlite-wal"), self.0.with_extension("sqlite-shm")] {
            let _ = fs::remove_file(path);
        }
    }
}
fn legacy() -> ProviderConfig {
    ProviderConfig { version: 41, region: Some("beijing".into()), workspace: Some("existing-workspace".into()),
        asr_model: "custom-asr-model".into(), rewrite_model: "qwen-flash".into(),
        adapter_version: "existing-adapter".into(), ..ProviderConfig::default() }
}
fn marker_count(connection: &Connection) -> u64 {
    connection.query_row("SELECT COUNT(*) FROM app_upgrades WHERE key=?1", [UPGRADE], |row| row.get(0)).unwrap()
}
fn assert_versioned_config(store: &Store, expected: &ProviderConfig) {
    let (version, data): (u64, String) = store.test_connection().query_row(
        "SELECT version,data FROM provider_config WHERE singleton=1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(version, expected.version);
    assert_eq!(serde_json::from_str::<ProviderConfig>(&data).unwrap(), *expected);
    assert_eq!(store.provider_config().unwrap(), *expected);
    assert_eq!(store.test_connection().pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0)).unwrap(), 3);
}
fn assert_no_upgrade_table(store: &Store) {
    assert_eq!(store.test_connection().query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='app_upgrades'", [],
        |row| row.get::<_, u64>(0)).unwrap(), 0);
}

#[test]
fn model_upgrade_new_profile_uses_new_default_without_incrementing_its_version() {
    let config = ProviderConfig::default();
    assert_eq!(config.rewrite_model, "qwen3.8-flash");
    assert_eq!(config.region, None);
    let file = UpgradeDb::new(&config);
    let mut store = file.open();
    assert_no_upgrade_table(&store);
    store.apply_app_upgrades().unwrap();
    assert_versioned_config(&store, &config);
    assert_eq!(marker_count(store.test_connection()), 1);
}

#[test]
fn model_upgrade_store_open_does_not_change_an_existing_profile() {
    let old = legacy();
    let file = UpgradeDb::new(&old);
    for _ in 0..2 {
        let store = file.open();
        assert_versioned_config(&store, &old);
        assert_no_upgrade_table(&store);
    }
}

#[test]
fn model_upgrade_updates_only_model_and_version_and_persists_once() {
    let old = legacy();
    let file = UpgradeDb::new(&old);
    let mut expected = old.clone();
    expected.rewrite_model = "qwen3.8-flash".into();
    expected.version += 1;
    {
        let mut store = file.open();
        store.apply_app_upgrades().unwrap();
        assert_versioned_config(&store, &expected);
        assert_eq!(marker_count(store.test_connection()), 1);
    }
    let mut reopened = file.open();
    reopened.apply_app_upgrades().unwrap();
    assert_versioned_config(&reopened, &expected);
    assert_eq!(marker_count(reopened.test_connection()), 1);
}

#[test]
fn model_upgrade_skips_other_models_regions_and_providers_permanently() {
    let cases = [
        ProviderConfig { region: None, ..legacy() },
        ProviderConfig { region: Some("singapore".into()), ..legacy() },
        ProviderConfig { region: Some("other-region".into()), ..legacy() },
        ProviderConfig { provider: "other-provider".into(), ..legacy() },
        ProviderConfig { rewrite_model: "qwen-plus".into(), ..legacy() },
        ProviderConfig { rewrite_model: "qwen3.8-flash".into(), ..legacy() },
    ];
    for old in cases {
        let file = UpgradeDb::new(&old);
        let mut store = file.open();
        store.apply_app_upgrades().unwrap();
        assert_versioned_config(&store, &old);
        assert_eq!(marker_count(store.test_connection()), 1);
        // A user can later explicitly select the former default without triggering an upgrade.
        let selected = store.save_provider_config(legacy(), old.version).unwrap();
        drop(store);
        let mut reopened = file.open();
        reopened.apply_app_upgrades().unwrap();
        assert_versioned_config(&reopened, &selected);
    }
}

#[test]
fn model_upgrade_marker_failure_rolls_back_model_and_both_versions_then_can_retry() {
    let old = legacy();
    let file = UpgradeDb::new(&old);
    let mut store = file.open();
    store.test_connection().execute_batch("CREATE TABLE app_upgrades(key TEXT PRIMARY KEY NOT NULL);
        CREATE TRIGGER reject_upgrade_marker BEFORE INSERT ON app_upgrades BEGIN SELECT RAISE(ABORT,'test marker failure'); END;").unwrap();
    assert!(matches!(store.apply_app_upgrades(), Err(StoreError::Database(_))));
    assert!(store.test_connection().is_autocommit());
    assert_versioned_config(&store, &old);
    assert_eq!(marker_count(store.test_connection()), 0);
    drop(store);
    let mut reopened = file.open();
    assert_versioned_config(&reopened, &old);
    reopened.test_connection().execute_batch("DROP TRIGGER reject_upgrade_marker;").unwrap();
    reopened.apply_app_upgrades().unwrap();
    let expected = ProviderConfig { version: old.version + 1, rewrite_model: "qwen3.8-flash".into(), ..old };
    assert_versioned_config(&reopened, &expected);
    assert_eq!(marker_count(reopened.test_connection()), 1);
}

#[test]
fn model_upgrade_config_failure_rolls_back_additive_table_creation() {
    let old = legacy();
    let file = UpgradeDb::new(&old);
    let mut store = file.open();
    store.test_connection().execute_batch("CREATE TRIGGER reject_model_update BEFORE UPDATE ON provider_config
        BEGIN SELECT RAISE(ABORT,'test config failure'); END;").unwrap();
    assert!(matches!(store.apply_app_upgrades(), Err(StoreError::Database(_))));
    assert!(store.test_connection().is_autocommit());
    assert_versioned_config(&store, &old);
    assert_no_upgrade_table(&store);
}

#[test]
fn model_upgrade_does_not_repair_or_mark_a_mismatched_stored_version() {
    let old = legacy();
    let file = UpgradeDb::new(&old);
    let mut store = file.open();
    store.test_connection().execute("UPDATE provider_config SET version=42 WHERE singleton=1", []).unwrap();
    assert!(matches!(store.apply_app_upgrades(), Err(StoreError::Serialization(_))));
    assert_eq!(store.provider_config().unwrap(), old);
    assert_eq!(store.test_connection().query_row("SELECT version FROM provider_config", [], |row| row.get::<_, u64>(0)).unwrap(), 42);
    assert_no_upgrade_table(&store);
}
