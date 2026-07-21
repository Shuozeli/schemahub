//! Mutation orchestration (design.md §5): single mutations, transactions, the
//! idempotency edge, the compatibility gate, and the codegen import closure.

pub mod closure;
pub mod compat;
pub mod idempotency;
pub mod single;
pub mod transaction;

use schemahub_jj::{Jj, JjError, RefSpec};
use schemahub_types::SchemaObjects;

use crate::error::CoreResult;
use crate::Core;

impl Core {
    /// Validate an advisory write base. A supplied commit must belong to this
    /// repository's retained history; stale ancestors remain valid because JJ
    /// merges the writer's result with the live bookmark instead of CAS-rejecting.
    pub fn validate_base_revision(
        &self,
        project: &str,
        repo: &str,
        base_revision: Option<&str>,
    ) -> CoreResult<()> {
        if let Some(commit) = base_revision {
            self.jj.validate_revision(project, repo, commit)?;
        }
        Ok(())
    }
}

/// Load a schema's objects for use as a mutation base, tolerating a not-yet-
/// existing bookmark or schema file (returns an empty [`SchemaObjects`]). This
/// makes "create the first schema on a fresh bookmark" just work: the compiler's
/// create/add ops produce upserts against an empty base.
///
/// Crucially: only the "ref/file is genuinely missing" variants
/// (`BookmarkNotFound` / `SchemaNotFound` / `TagNotFound`) become an empty
/// base. Every other `JjError` (`Corrupt`, `ObjectDb`, `BadRef`, …) is
/// propagated — silently swallowing those would let a broken JJ look like
/// a fresh bookmark and overwrite real content.
pub fn load_base(
    jj: &Jj,
    project: &str,
    repo: &str,
    schema_path: &str,
    base_ref: &RefSpec,
) -> CoreResult<SchemaObjects> {
    match jj.load_schema(project, repo, schema_path, base_ref) {
        Ok(objs) => Ok(objs),
        Err(JjError::BookmarkNotFound(_))
        | Err(JjError::SchemaNotFound(_))
        | Err(JjError::TagNotFound(_)) => Ok(SchemaObjects::default()),
        Err(e) => Err(e.into()),
    }
}

/// Resolve a mutable bookmark once and return the immutable commit used for
/// both planning and publication.
///
/// Passing a bookmark through to the eventual JJ commit would resolve it a
/// second time after parsing/validation. A concurrent writer could then become
/// the apparent base and be overwritten instead of participating in JJ's
/// three-way merge. A missing bookmark resolves to the repository root, which
/// preserves first-write behavior while still giving the write a stable base.
pub(crate) fn immutable_bookmark_base(
    jj: &Jj,
    project: &str,
    repo: &str,
    bookmark: &str,
) -> CoreResult<RefSpec> {
    Ok(RefSpec::commit(jj.resolve_ref_or_root(
        project,
        repo,
        &RefSpec::bookmark(bookmark),
    )?))
}
