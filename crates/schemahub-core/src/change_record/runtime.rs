use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChangeRuntimeError {
    #[error("system clock is before the Unix epoch: {0}")]
    Clock(String),
    #[error("timestamp does not fit in signed milliseconds")]
    TimestampOverflow,
}

/// Injected source of audit time for deterministic lifecycle tests.
pub trait ChangeClock: Send + Sync + 'static {
    fn now_unix_millis(&self) -> Result<i64, ChangeRuntimeError>;
}

/// Injected source of resource IDs for deterministic lifecycle tests.
pub trait ChangeIdGenerator: Send + Sync + 'static {
    fn generate_change_id(&self) -> String;

    fn generate_apply_attempt_id(&self) -> String {
        self.generate_change_id()
    }

    fn generate_apply_lease_owner(&self) -> String {
        self.generate_change_id()
    }
}

#[derive(Debug, Default)]
pub struct SystemChangeClock;

impl ChangeClock for SystemChangeClock {
    fn now_unix_millis(&self) -> Result<i64, ChangeRuntimeError> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ChangeRuntimeError::Clock(error.to_string()))?
            .as_millis();
        i64::try_from(millis).map_err(|_| ChangeRuntimeError::TimestampOverflow)
    }
}

#[derive(Debug, Default)]
pub struct UuidChangeIdGenerator;

impl ChangeIdGenerator for UuidChangeIdGenerator {
    fn generate_change_id(&self) -> String {
        format!("chg-{}", uuid::Uuid::new_v4().simple())
    }

    fn generate_apply_attempt_id(&self) -> String {
        format!("apply-{}", uuid::Uuid::new_v4().simple())
    }

    fn generate_apply_lease_owner(&self) -> String {
        format!("lease-{}", uuid::Uuid::new_v4().simple())
    }
}
