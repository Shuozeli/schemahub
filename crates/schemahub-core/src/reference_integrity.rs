//! Repository-snapshot reference-integrity checks.
//!
//! Whole-schema deletion is safe only when every remaining schema's live,
//! unpinned import has been updated or removed in the same final-state plan.
//! Immutable pins remain valid because their target commit stays retained even
//! when a mutable bookmark no longer contains the schema.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use schemahub_jj::{PublicationSnapshot, RefSpec};
use schemahub_types::{SchemaObjects, SchemaPath};

use crate::error::{CoreError, CoreResult};
use crate::exploration::normalize_import_path;
use crate::Core;

impl Core {
    /// Validate policy against the exact tree that JJ is about to publish.
    /// This method is invoked while the repository publication lock is held.
    /// Protected bookmarks reject every unresolved final-tree conflict even
    /// when compatibility was force-overridden. Whenever a schema present in
    /// a publication input disappears from the final tree, every remaining
    /// unpinned same-repository import is checked against that disappearance.
    pub(crate) fn validate_publication_snapshot(
        &self,
        project: &str,
        repo: &str,
        bookmark: &str,
        protected: bool,
        snapshot: &PublicationSnapshot<'_>,
    ) -> CoreResult<()> {
        if protected && snapshot.bookmark_target_conflicted() {
            return Err(CoreError::FailedPrecondition(format!(
                "protected bookmark {bookmark:?} cannot publish a conflicted bookmark target"
            )));
        }
        let conflicts = snapshot.conflicted_declarations()?;
        if protected && !conflicts.is_empty() {
            return Err(CoreError::FailedPrecondition(format!(
                "protected bookmark {bookmark:?} cannot publish unresolved conflicts: {}",
                conflicts.join(", ")
            )));
        }

        let final_schemas: BTreeSet<_> = snapshot.list_schemas()?.into_iter().collect();
        let known_schemas = snapshot.known_schema_names();
        let disappeared: BTreeSet<_> = known_schemas.difference(&final_schemas).cloned().collect();
        if disappeared.is_empty() {
            return Ok(());
        }

        if let Some(conflict) = conflicts.iter().find(|path| path.ends_with("/__meta__")) {
            return Err(CoreError::FailedPrecondition(format!(
                "cannot prove reference integrity while {conflict} is conflicted"
            )));
        }

        let resolution_schemas: HashSet<_> = known_schemas.iter().cloned().collect();
        let mut dangling = Vec::new();
        for schema_name in &final_schemas {
            let schema = snapshot.load_schema(schema_name)?;
            let compiler = self.compiler_for(schema_name)?;
            for import in compiler.imports(&schema)? {
                if !import.resolved_commit.is_empty() {
                    continue;
                }
                let importing_schema = SchemaPath::new(project, repo, schema_name);
                let target =
                    normalize_import_path(&importing_schema, &import.path, &resolution_schemas)?;
                if target.project == project
                    && target.repo == repo
                    && disappeared.contains(&target.schema_name)
                {
                    dangling.push(format!("{schema_name} -> {}", target.schema_name));
                }
            }
        }
        dangling.sort();
        dangling.dedup();
        if dangling.is_empty() {
            Ok(())
        } else {
            Err(CoreError::FailedPrecondition(format!(
                "final repository state contains live unpinned imports to deleted schemas: {}",
                dangling.join(", ")
            )))
        }
    }

    /// Find schemas that would retain a live unpinned import to `target` in the
    /// proposed final state. `overrides` maps touched schema names to their
    /// final state (`None` means deleted); untouched schemas are loaded from
    /// `at`.
    pub(crate) fn live_unpinned_dependents(
        &self,
        target: &SchemaPath,
        at: &RefSpec,
        overrides: &BTreeMap<String, Option<SchemaObjects>>,
    ) -> CoreResult<Vec<String>> {
        let conflicts = self
            .jj
            .list_conflicted_declarations(&target.project, &target.repo, at)?;
        let mut schema_names: BTreeSet<_> = self
            .jj
            .list_schemas(&target.project, &target.repo, at)?
            .into_iter()
            .collect();
        schema_names.extend(overrides.keys().cloned());
        let resolution_schemas: HashSet<_> = schema_names.iter().cloned().collect();

        let mut dependents = Vec::new();
        for schema_name in schema_names {
            if schema_name == target.schema_name {
                continue;
            }
            let schema = match overrides.get(&schema_name) {
                Some(Some(schema)) => schema.clone(),
                Some(None) => continue,
                None => {
                    if conflicts
                        .iter()
                        .any(|path| path == &format!("{schema_name}/__meta__"))
                    {
                        return Err(CoreError::FailedPrecondition(format!(
                            "cannot prove reference integrity while {schema_name} has conflicted metadata"
                        )));
                    }
                    self.jj
                        .load_schema(&target.project, &target.repo, &schema_name, at)?
                }
            };
            let compiler = self.compiler_for(&schema_name)?;
            let imports = compiler.imports(&schema)?;
            let importing_schema = SchemaPath::new(&target.project, &target.repo, &schema_name);
            let mut imports_target = false;
            for import in imports {
                if import.resolved_commit.is_empty()
                    && normalize_import_path(&importing_schema, &import.path, &resolution_schemas)?
                        == *target
                {
                    imports_target = true;
                    break;
                }
            }
            if imports_target {
                dependents.push(schema_name);
            }
        }
        Ok(dependents)
    }
}
