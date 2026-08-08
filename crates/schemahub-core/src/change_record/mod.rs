//! Durable schema-change intent and lifecycle orchestration.
//!
//! This module is deliberately independent of tonic and the compiler-specific
//! mutation encodings. It owns the format-agnostic resource model, injected
//! time/ID dependencies, optimistic-concurrency store contract, and lifecycle
//! rules described in `docs/change-records.md`.

mod ledger;
mod model;
mod runtime;
mod store;
pub(crate) mod validation;

pub use ledger::{ApplyAcquisition, ChangeLedger, ChangeLedgerError};
pub use model::{
    ApplyAttempt, ApplyResult, ChangeActor, ChangeEdit, ChangeRecord, ChangeRecordStatus,
    ChangeReview, ChangeReviewDecision, ChangeUpdate, CreateChange, ValidationIssue,
    ValidationResult,
};
pub use runtime::{
    ChangeClock, ChangeIdGenerator, ChangeRuntimeError, SystemChangeClock, UuidChangeIdGenerator,
};
pub use store::{
    ChangeRecordPage, ChangeRecordPageCursor, ChangeRecordStore, ChangeStoreError,
    MemoryChangeRecordStore, ObjectDbChangeRecordStore,
};

#[cfg(test)]
mod tests;
