//! Pure domain/storage layer. Call from one DB worker; never hold this store across network awaits.
pub mod commit_guard;
pub mod db_worker;
pub use commit_guard::{CommitGuard,CancelDecision};
pub use db_worker::{DbClient,DbCommand,DbValue,DbSnapshot,DbTicket,DbPriority,DbOptions};
pub mod store;
pub mod onboarding;
pub use onboarding::{OnboardingProgress, OnboardingStep, OnboardingLesson};
pub mod types;
pub mod app_writing;
pub mod personalization;
pub use personalization::*;
pub mod confirmed_context;
pub mod writing_context;
pub use confirmed_context::{ConfirmedContext, DraftLanguage, SourceCorrection};
pub use app_writing::{AppWritingPreferences, AppWritingOverride, AppWritingScene};
pub use store::{Store, StoreError, StoreResult};
pub use types::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod db_worker_tests;

#[cfg(test)]
mod model_upgrade_tests;
