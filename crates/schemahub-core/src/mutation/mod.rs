//! Mutation orchestration (design.md §5): single mutations, transactions, the
//! idempotency edge, the compatibility gate, and the codegen import closure.

pub mod closure;
pub mod compat;
pub mod idempotency;
pub mod single;
pub mod transaction;

use schemahub_types::SchemaObjects;
use schemahub_vcs::{RefSpec, Vcs, VcsError};

use crate::error::CoreResult;

/// Load a schema's objects for use as a mutation base, tolerating a not-yet-
/// existing bookmark or schema file (returns an empty [`SchemaObjects`]). This
/// makes "create the first schema on a fresh bookmark" just work: the compiler's
/// create/add ops produce upserts against an empty base.
pub(crate) fn load_base(
    vcs: &Vcs,
    project: &str,
    repo: &str,
    schema_path: &str,
    base_ref: &RefSpec,
) -> CoreResult<SchemaObjects> {
    match vcs.load_schema(project, repo, schema_path, base_ref) {
        Ok(objs) => Ok(objs),
        Err(VcsError::BookmarkNotFound(_))
        | Err(VcsError::SchemaNotFound(_))
        | Err(VcsError::TagNotFound(_)) => Ok(SchemaObjects::default()),
        Err(e) => Err(e.into()),
    }
}
