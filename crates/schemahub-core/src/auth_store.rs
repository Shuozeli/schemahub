//! Project + role registries — the data the real `AuthzPolicy` and the
//! `ProjectService` RPCs both read from (design.md §6).
//!
//! Two small traits, decoupled from the [`ObjectDb`](schemahub_vcs::ObjectDb)
//! deliberately: the postgres-backed `ObjectDb` impl just landed and we keep
//! that surface stable. Roles and project metadata live in their own store
//! (the default is a file-backed JSON impl — see [`auth_files`](crate::auth_files));
//! a future deployment can swap in a SQL impl without touching the VCS layer.

use schemahub_types::{Identity, Role, Visibility};

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
}

/// Role registry — per-project map from `Identity` to [`Role`]. The default
/// impl is file-backed JSON (see [`crate::auth_files::FileRoleStore`]).
///
/// Implementations must be safe to call from many concurrent gRPC handlers —
/// the trait is `Send + Sync`. Mutations should be durable before returning
/// (the file-backed impl uses an atomic tempfile + rename).
pub trait RoleStore: Send + Sync + 'static {
    /// Lookup the caller's role on `project`. Returns `None` if the identity is
    /// anonymous or has no assigned role.
    fn get(&self, project: &str, identity: &Identity) -> Option<Role>;

    /// Set / replace the role for `identity_id` on `project`. Used by AddMember
    /// and UpdateMemberRole, and by the [`crate::auth_files`] bootstrap loader.
    fn set(&self, project: &str, identity_id: &str, role: Role) -> std::io::Result<()>;

    /// Remove the role for `identity_id` on `project`. No-op (and `Ok`) if the
    /// identity has no role on the project.
    fn remove(&self, project: &str, identity_id: &str) -> std::io::Result<()>;

    /// List every `(identity_id, role)` on the project. Empty when the project
    /// has no members. Order is unspecified.
    fn list_project(&self, project: &str) -> Vec<(String, Role)>;
}

/// Project registry — keeps each project's `ProjectMeta`. The default impl is
/// file-backed JSON (see [`crate::auth_files::FileProjectStore`]).
pub trait ProjectStore: Send + Sync + 'static {
    /// Lookup project metadata; `None` if the project does not exist.
    fn get(&self, project: &str) -> Option<ProjectMeta>;

    /// Create or replace project metadata. Used by `CreateProject` and by the
    /// `[projects.<name>]` bootstrap loader.
    fn set(&self, meta: ProjectMeta) -> std::io::Result<()>;

    /// List every known project. Order is unspecified.
    fn list(&self) -> Vec<ProjectMeta>;
}
