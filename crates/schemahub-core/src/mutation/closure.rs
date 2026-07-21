//! Transitive import-closure computation for codegen and serving.
//!
//! Every queued schema is paired with an immutable, repository-owned commit.
//! Same-repository live imports stay on their importing snapshot; live
//! cross-repository imports resolve the target repository's configured default
//! bookmark once; stored pins remain immutable. A single closure cannot contain
//! two revisions of the same logical schema, so that condition fails closed.

use std::collections::{HashMap, HashSet};

use schemahub_jj::RefSpec;
use schemahub_types::{SchemaClosure, SchemaPath};

use crate::error::{CoreError, CoreResult};
use crate::exploration::normalize_import_path;
use crate::Core;

/// Build the transitive closure rooted at one already-resolved commit.
pub(crate) fn build(
    core: &Core,
    root: &SchemaPath,
    root_commit: &str,
    token: Option<&str>,
) -> CoreResult<SchemaClosure> {
    let root_format = core.compiler_for(&root.schema_name)?.format_id();
    let mut closure = SchemaClosure::with_root(root.clone());
    let mut revisions: HashMap<SchemaPath, String> = HashMap::new();
    let mut live_snapshots = HashMap::new();
    let mut queue = vec![(root.clone(), root_commit.to_string())];

    while let Some((path, requested_commit)) = queue.pop() {
        if let Some(existing) = revisions.get(&path) {
            if existing == &requested_commit {
                continue;
            }
            return Err(CoreError::FailedPrecondition(format!(
                "schema closure requires two revisions of {path}: {existing} and {requested_commit}"
            )));
        }

        // Revalidate every immutable coordinate at the repository boundary.
        // This also authorizes the target repository and rejects archived repos.
        let commit = core.resolve_read_commit(
            &path.project,
            &path.repo,
            &RefSpec::commit(requested_commit),
            token,
        )?;
        revisions.insert(path.clone(), commit.clone());

        let compiler = core.compiler_for(&path.schema_name)?;
        if compiler.format_id() != root_format {
            return Err(CoreError::FailedPrecondition(format!(
                "schema closure rooted at {} cannot include different format {}",
                root.schema_name, path.schema_name
            )));
        }
        let at = RefSpec::commit(commit.clone());
        let objects = core
            .jj
            .load_schema(&path.project, &path.repo, &path.schema_name, &at)?;
        let imports = compiler.imports(&objects)?;
        let same_repo_schemas: HashSet<String> = core
            .jj
            .list_schemas(&path.project, &path.repo, &at)?
            .into_iter()
            .collect();
        closure.entries.insert(path.clone(), objects);

        for import in imports {
            let dependency = normalize_import_path(&path, &import.path, &same_repo_schemas)?;
            let dependency_commit = core.resolve_import_commit(
                &path,
                &commit,
                &dependency,
                &import,
                token,
                &mut live_snapshots,
            )?;
            queue.push((dependency, dependency_commit));
        }
    }

    Ok(closure)
}
