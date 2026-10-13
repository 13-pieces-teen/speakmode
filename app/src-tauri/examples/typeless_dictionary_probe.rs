//! Explicit local-source verification. No saved SpeakMode profile or term output.
use speakmode_lib::{core::Store, platform::typeless_dictionary};

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    anyhow::ensure!(args == ["--read-local-cache"], "Pass --read-local-cache to read this user's Typeless dictionary cache; no credentials, network, or saved SpeakMode profile are used.");
    let imported = typeless_dictionary::read_local()
        .map_err(|error| anyhow::anyhow!("{}: {}", error.code(), error.message()))?;
    let source_count = imported.terms.len();
    let mut store = Store::in_memory()?;
    let saved = store.import_dictionary(imported.terms.clone(), 1)?;
    let repeated = store.import_dictionary(imported.terms, saved.version)?;
    anyhow::ensure!(saved == repeated, "Repeated import must preserve the dictionary and version");
    println!("{}", serde_json::json!({
        "scope": "real local Typeless cache read; SpeakMode import and repeat check in isolated in-memory SQLite only",
        "sourceWords": source_count,
        "importedWords": saved.entries.len(),
        "repeatedImportAdded": 0,
        "cacheDate": imported.cached_at,
        "termContentsPrinted": false,
        "existingSpeakModeProfileWritten": false,
        "credentialsAccessed": false,
        "cloudRequests": 0,
    }));
    Ok(())
}
