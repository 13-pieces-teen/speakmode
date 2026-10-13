use super::{dictionary_error, ApiError, AppHub, DbCommand, Result, StoreError, Value};

impl AppHub {
    pub(super) async fn import_typeless_dictionary(&self, expected_version: u64) -> Result<Value> {
        self.import_typeless_dictionary_with(expected_version, || {
            crate::platform::typeless_dictionary::read_local()
                .map(|imported| imported.terms)
                .map_err(|error| ApiError::new(error.code(), "dictionary", error.message()))
        })
        .await
    }

    /// An explicit dictionary import reads only after rejecting a stale draft.
    /// Blocking source I/O never holds the session lock or database owner.
    /// The Store remains authoritative for normalization, capacity and the
    /// version check after reading; no source word can be committed partially.
    pub(super) async fn import_typeless_dictionary_with<F>(
        &self,
        expected_version: u64,
        reader: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Result<Vec<String>> + Send + 'static,
    {
        let current = self.db.snapshot().dictionary.version;
        if current != expected_version {
            return Err(dictionary_error(StoreError::Conflict {
                current: current.to_string(),
            }));
        }
        let terms = tokio::task::spawn_blocking(reader)
            .await
            .map_err(|_| {
                ApiError::new(
                    "typeless_import_unavailable",
                    "dictionary",
                    "暂时无法读取 Typeless 词典，请稍后重试。",
                )
            })??;
        self.dictionary_write(DbCommand::ImportDictionary {
            terms,
            expected_version,
        })
        .await
    }
}
