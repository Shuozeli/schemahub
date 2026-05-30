//! Transitive import-closure computation for codegen (design.md §10).
//!
//! BFS over the `imports` declared in each schema file's `__meta__`, resolving
//! every import's pinned `resolved_commit` via `vcs.load_schema(.. Commit ..)`,
//! with cycle detection. Produces a [`SchemaClosure`] the compiler turns into
//! descriptors / generated code.
//!
//! Imports carry a logical `project/repo/schema` path; we parse that back into a
//! [`SchemaPath`] so cross-repo imports resolve against the right repo. An
//! import with an empty `resolved_commit` is resolved against the same ref as
//! the root (the common single-repo case where pins are not yet populated).

use std::collections::HashSet;

use schemahub_types::{Compiler, SchemaClosure, SchemaPath};
use schemahub_vcs::{RefSpec, Vcs};

use crate::error::CoreResult;

/// Build the transitive closure rooted at `root` resolved at `root_ref`.
pub(crate) fn build(
    vcs: &Vcs,
    compiler: &dyn Compiler,
    root: &SchemaPath,
    root_ref: &RefSpec,
) -> CoreResult<SchemaClosure> {
    let mut closure = SchemaClosure::new();
    let mut visited: HashSet<SchemaPath> = HashSet::new();
    // Queue holds (schema, ref-to-resolve-it-at).
    let mut queue: Vec<(SchemaPath, RefSpec)> = vec![(root.clone(), root_ref.clone())];

    while let Some((path, at)) = queue.pop() {
        if !visited.insert(path.clone()) {
            continue; // cycle / already resolved
        }

        let objs = vcs.load_schema(&path.project, &path.repo, &path.schema_name, &at)?;
        let imports = compiler.imports(&objs.meta)?;
        closure.entries.insert(path.clone(), objs);

        for import in imports {
            let Some(dep_path) = parse_import_path(&import.path) else {
                continue; // malformed/unresolvable logical path — skip, don't fail closure
            };
            if visited.contains(&dep_path) {
                continue;
            }
            // Pinned commit if present, else the root's ref (same-repo default).
            let dep_ref = if import.resolved_commit.is_empty() {
                at.clone()
            } else {
                RefSpec::commit(import.resolved_commit.clone())
            };
            queue.push((dep_path, dep_ref));
        }
    }

    Ok(closure)
}

/// Parse a logical import path "project/repo/schema-file" into a [`SchemaPath`].
/// The schema-file component may itself contain '/', so we split on the first
/// two separators only.
fn parse_import_path(path: &str) -> Option<SchemaPath> {
    let mut parts = path.splitn(3, '/');
    let project = parts.next()?;
    let repo = parts.next()?;
    let schema = parts.next()?;
    if project.is_empty() || repo.is_empty() || schema.is_empty() {
        return None;
    }
    Some(SchemaPath::new(project, repo, schema))
}

