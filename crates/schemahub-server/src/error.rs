use schemahub_core::CoreError;
use tonic::Status;

/// Convert a CoreError into a tonic Status.
pub fn core_to_status(e: CoreError) -> Status {
    match e {
        CoreError::NotFound(msg) => Status::not_found(msg),
        CoreError::AlreadyExists(msg) => Status::already_exists(msg),
        CoreError::InvalidArgument(msg) => Status::invalid_argument(msg),
        CoreError::PermissionDenied(msg) => Status::permission_denied(msg),
        CoreError::Unauthenticated(msg) => Status::unauthenticated(msg),
        CoreError::Conflict { current_head, provided_base } => Status::failed_precondition(
            format!(
                "conflict: current HEAD is {current_head}, but provided base is {provided_base}"
            ),
        ),
        CoreError::CompatibilityViolation(violations) => {
            let msgs: Vec<String> = violations.iter().map(|v| v.message.clone()).collect();
            Status::failed_precondition(format!("compatibility violations: {}", msgs.join("; ")))
        }
        CoreError::MutationError(e) => Status::invalid_argument(e.to_string()),
        CoreError::Storage(e) => Status::internal(e.to_string()),
        CoreError::ObjectCorrupted(msg) => Status::internal(format!("object corrupted: {msg}")),
    }
}
