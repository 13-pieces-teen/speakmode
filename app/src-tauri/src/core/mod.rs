//! Pure domain/storage layer. Call from one DB worker; never hold this store across network awaits.
pub mod commit_guard;
pub mod db_worker;
pub use commit_guard::{CommitGuard,CancelDecision};
pub use db_worker::{DbClient,DbCommand,DbValue,DbSnapshot,DbTicket,DbPriority,DbOptions};
pub mod store;
pub mod types;
pub use store::{Store, StoreError, StoreResult};
pub use types::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod db_worker_tests;

#[cfg(test)]
mod model_upgrade_tests;
