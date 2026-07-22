//! Project + role registries — the data the real `AuthzPolicy` and the
//! `ProjectService` RPCs both read from (design.md §6).
//!
//! The traits keep authorization independent from storage. Production uses
//! ObjectDb-backed implementations on redb or PostgreSQL; the JSON stores in
//! [`auth_files`](crate::auth_files) remain only as legacy migration readers.

use schemahub_types::{Identity, Role, Visibility};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccessStoreError {
    #[error("resource already exists: {0}")]
    AlreadyExists(String),
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("resource etag mismatch for {name}: expected {expected}, current {current}")]
    EtagMismatch {
        name: String,
        expected: String,
        current: String,
    },
    #[error("access store error: {0}")]
    Backend(String),
}

pub type AccessStoreResult<T> = Result<T, AccessStoreError>;

/// Project-level metadata: visibility flag + the original creator (recorded as
/// the bootstrap Owner). The owners list is not persisted here — that lives in
/// the [`RoleStore`] keyed `(project, identity_id)`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub visibility: Visibility,
    /// The identity_id of the first Owner — recorded for audit / ProjectInfo.owner.
    /// May be empty when the project was created anonymously (Noop auth).
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub etag: String,
    #[serde(default)]
    pub create_time_unix_ms: i64,
    #[serde(default)]
    pub update_time_unix_ms: i64,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub archive_time_unix_ms: Option<i64>,
}

impl ProjectMeta {
    pub fn new(
        name: impl Into<String>,
        visibility: Visibility,
        creator: impl Into<String>,
        now_unix_ms: i64,
    ) -> Self {
        Self {
            name: name.into(),
            visibility,
            creator: creator.into(),
            etag: String::new(),
            create_time_unix_ms: now_unix_ms,
            update_time_unix_ms: now_unix_ms,
            archived: false,
            archive_time_unix_ms: None,
        }
    }
}

/// Role registry — per-project map from `Identity` to [`Role`].
///
/// Implementations must be safe to call from many concurrent gRPC handlers —
/// the trait is `Send + Sync`. Mutations should be durable before returning
/// before returning.
pub trait RoleStore: Send + Sync + 'static {
    /// Lookup the caller's role on `project`. Returns `None` if the identity is
    /// anonymous or has no assigned role.
    fn get(&self, project: &str, identity: &Identity) -> AccessStoreResult<Option<Role>>;

    /// Set / replace the role for `identity_id` on `project`. Used by AddMember
    /// and UpdateMemberRole, and by the [`crate::auth_files`] bootstrap loader.
    fn set(&self, project: &str, identity_id: &str, role: Role) -> AccessStoreResult<()>;

    /// Remove the role for `identity_id` on `project`. No-op (and `Ok`) if the
    /// identity has no role on the project.
    fn remove(&self, project: &str, identity_id: &str) -> AccessStoreResult<()>;

    /// List every `(identity_id, role)` on the project. Empty when the project
    /// has no members. Order is unspecified.
    fn list_project(&self, project: &str) -> AccessStoreResult<Vec<(String, Role)>>;
}

/// Project registry — keeps each project's durable `ProjectMeta` resource.
pub trait ProjectStore: Send + Sync + 'static {
    /// Lookup project metadata; `None` if the project does not exist.
    fn get(&self, project: &str) -> AccessStoreResult<Option<ProjectMeta>>;

    /// Atomically create project metadata and its initial Owner membership.
    fn create_with_owner(
        &self,
        meta: ProjectMeta,
        owner_id: &str,
    ) -> AccessStoreResult<ProjectMeta>;

    /// Create or replace project metadata. Used by `CreateProject` and by the
    /// `[projects.<name>]` bootstrap loader.
    fn set(&self, meta: ProjectMeta) -> AccessStoreResult<()>;

    /// Optimistically replace project metadata.
    fn replace(&self, expected_etag: &str, meta: ProjectMeta) -> AccessStoreResult<ProjectMeta>;

    /// List every known project. Order is unspecified.
    fn list(&self) -> AccessStoreResult<Vec<ProjectMeta>>;
}
