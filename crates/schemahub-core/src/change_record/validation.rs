//! Deterministic, side-effect-free validation of executable change records.
//!
//! Validation resolves a mutable target to one immutable JJ commit, replays the
//! ordered edits in memory through the registered compiler, runs protected-ref
//! compatibility policy, and returns both a persisted report and an internal
//! write plan. Only the report is stored; Apply rebuilds the plan and verifies
//! the same digest/base before writing.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use schemahub_jj::{JjError, RefSpec};
use schemahub_types::{Mutation, MutationEffect, SchemaObjects};
use sha2::{Digest, Sha256};

use super::{ChangeEdit, ChangeRecord, ValidationIssue, ValidationResult};
use crate::mutation::compat;
use crate::request::TransactionLimits;
use crate::{detect_format_from_name, Core, CoreError, CoreResult};

pub(crate) const VALIDATOR_VERSION: &str = "schemahub-change-validator/v1";

#[derive(Clone, Debug)]
pub(crate) enum PreparedSchemaChange {
    Patch {
        schema_name: String,
        effect: MutationEffect,
    },
    Delete {
        schema_name: String,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedChange {
    pub resolved_base_commit: String,
    pub edit_digest: String,
    pub writes: Vec<PreparedSchemaChange>,
}

impl PreparedChange {
    pub(crate) fn matches(&self, result: &ValidationResult) -> bool {
        self.resolved_base_commit == result.resolved_base_commit
            && self.edit_digest == result.edit_digest
            && !self.writes.is_empty()
            && self.writes.iter().all(|write| match write {
                PreparedSchemaChange::Patch {
                    schema_name,
                    effect,
                } => !schema_name.is_empty() && !effect_is_empty(effect),
                PreparedSchemaChange::Delete { schema_name } => !schema_name.is_empty(),
            })
    }
}

pub(crate) struct ValidationOutcome {
    pub result: ValidationResult,
    pub prepared: Option<PreparedChange>,
}

struct SchemaState {
    schema_name: String,
    original: Option<SchemaObjects>,
    current: Option<SchemaObjects>,
    failed: bool,
}

/// Validate the current immutable snapshot of a change record without writing
/// schema state. Expected compiler/policy failures are returned as report data;
/// storage corruption and other infrastructure failures remain Core errors.
pub(crate) fn validate(core: &Core, record: &ChangeRecord) -> CoreResult<ValidationOutcome> {
    let edit_digest = edit_digest(&record.edits)?;
    let mut issues = structural_issues(core, record);

    let base_ref = match record.base_revision.as_deref() {
        Some(commit) => RefSpec::commit(commit),
        None => RefSpec::bookmark(record.target_bookmark.clone()),
    };
    let resolved_base_commit =
        match core
            .jj
            .resolve_ref_or_root(&record.project, &record.repo, &base_ref)
        {
            Ok(commit) => commit,
            Err(error) if expected_ref_error(&error) => {
                issues.push(issue(
                    "base_revision_unresolvable",
                    error.to_string(),
                    None,
                    None,
                ));
                String::new()
            }
            Err(error) => return Err(error.into()),
        };

    if !issues.is_empty() || resolved_base_commit.is_empty() {
        return Ok(outcome(resolved_base_commit, edit_digest, issues, None));
    }

    let format_id = edit_format(&record.edits).expect("structural validation checked edits");
    let compiler = core
        .registry
        .get(format_id)
        .expect("structural validation checked compiler")
        .clone();
    let immutable_base = RefSpec::commit(resolved_base_commit.clone());

    let touched: Vec<String> =
        record
            .edits
            .iter()
            .map(edit_schema_name)
            .fold(Vec::new(), |mut names, name| {
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_string());
                }
                names
            });

    let existing_conflicts =
        core.jj
            .list_conflicted_declarations(&record.project, &record.repo, &immutable_base)?;
    let mut states = Vec::with_capacity(touched.len());
    for schema_name in touched {
        let original =
            match core
                .jj
                .load_schema(&record.project, &record.repo, &schema_name, &immutable_base)
            {
                Ok(schema) => Some(schema),
                Err(JjError::SchemaNotFound(_)) => None,
                Err(error) => return Err(error.into()),
            };
        let conflicts: Vec<_> = existing_conflicts
            .iter()
            .filter_map(|path| {
                path.strip_prefix(&format!("{schema_name}/"))
                    .map(str::to_string)
            })
            .collect();
        for declaration in &conflicts {
            issues.push(issue(
                "base_conflict",
                "the validation base contains an unresolved declaration conflict",
                Some(&schema_name),
                Some(declaration),
            ));
        }
        states.push(SchemaState {
            schema_name,
            current: original.clone(),
            original,
            failed: !conflicts.is_empty(),
        });
    }

    for edit in &record.edits {
        let schema_name = edit_schema_name(edit);
        let state = states
            .iter_mut()
            .find(|state| state.schema_name == schema_name)
            .expect("state created for every edit");
        if state.failed {
            continue;
        }

        let result = match edit {
            ChangeEdit::Mutation {
                schema,
                format_id,
                operation,
            } => {
                let base = state.current.clone().unwrap_or_default();
                let mutation = Mutation {
                    schema_path: schema.clone(),
                    format_id: format_id.clone(),
                    operation: Bytes::from(operation.clone()),
                };
                compiler
                    .apply_mutation(&base, &mutation)
                    .map(|effect| Some(apply_effect(base, effect)))
                    .map_err(|error| ("mutation_invalid", error.to_string()))
            }
            ChangeEdit::ReplaceSource { source, .. } => compiler
                .parse(source)
                .map_err(|error| ("source_invalid", error.to_string()))
                .and_then(|parsed| {
                    let mut decls = BTreeMap::new();
                    for (name, blob) in parsed.decls {
                        if decls.insert(name.clone(), blob).is_some() {
                            return Err((
                                "duplicate_declaration",
                                format!("parsed source contains duplicate declaration {name:?}"),
                            ));
                        }
                    }
                    Ok(Some(SchemaObjects {
                        meta: parsed.meta,
                        decls,
                    }))
                }),
            ChangeEdit::DeleteSchema { .. } => {
                if state.current.is_none() {
                    Err((
                        "schema_not_found",
                        "cannot delete a schema that does not exist at this point in the edit sequence"
                            .to_string(),
                    ))
                } else {
                    Ok(None)
                }
            }
        };

        match result {
            Ok(next) => state.current = next,
            Err((code, message)) => {
                issues.push(issue(code, message, Some(schema_name), None));
                state.failed = true;
            }
        }
    }

    // Validate whole-schema deletions against the final state of every touched
    // schema. This lets one atomic change remove/update a consumer and then
    // delete its provider, while rejecting any untouched live import.
    if states.iter().all(|state| !state.failed) {
        let overrides: BTreeMap<_, _> = states
            .iter()
            .map(|state| (state.schema_name.clone(), state.current.clone()))
            .collect();
        for state in states
            .iter()
            .filter(|state| state.original.is_some() && state.current.is_none())
        {
            let target =
                schemahub_types::SchemaPath::new(&record.project, &record.repo, &state.schema_name);
            match core.live_unpinned_dependents(&target, &immutable_base, &overrides) {
                Ok(dependents) => {
                    for dependent in dependents {
                        issues.push(issue(
                            "live_schema_dependency",
                            format!(
                                "schema {dependent} retains an unpinned import to the deleted schema"
                            ),
                            Some(&state.schema_name),
                            None,
                        ));
                    }
                }
                Err(CoreError::FailedPrecondition(message)) => issues.push(issue(
                    "reference_integrity_unverifiable",
                    message,
                    Some(&state.schema_name),
                    None,
                )),
                Err(error) => return Err(error),
            }
        }
    }

    let config = core.effective_repo_config(&record.project, &record.repo)?;
    let protected =
        schemahub_jj::bookmark::is_protected(&record.target_bookmark, &config.protected_bookmarks);
    let mut writes = Vec::with_capacity(states.len());
    for state in states.into_iter().filter(|state| !state.failed) {
        match (&state.original, &state.current) {
            (Some(original), Some(current)) => {
                let effect = diff_effect(original, current);
                if effect_is_empty(&effect) {
                    issues.push(issue(
                        "no_effect",
                        "ordered edits leave the schema unchanged",
                        Some(&state.schema_name),
                        None,
                    ));
                    continue;
                }
                if protected {
                    collect_compatibility_issues(
                        compiler.as_ref(),
                        &config.compat_rules(),
                        original,
                        &effect,
                        &state.schema_name,
                        &mut issues,
                    )?;
                }
                writes.push(PreparedSchemaChange::Patch {
                    schema_name: state.schema_name,
                    effect,
                });
            }
            (None, Some(current)) => {
                let effect = diff_effect(&SchemaObjects::default(), current);
                if effect_is_empty(&effect) {
                    issues.push(issue(
                        "no_effect",
                        "ordered edits do not create any schema content",
                        Some(&state.schema_name),
                        None,
                    ));
                    continue;
                }
                writes.push(PreparedSchemaChange::Patch {
                    schema_name: state.schema_name,
                    effect,
                });
            }
            (Some(_), None) => {
                if protected && !config.compat_rules().disabled {
                    issues.push(issue(
                        "compatibility_violation",
                        "deleting an entire schema is blocked on a protected bookmark",
                        Some(&state.schema_name),
                        None,
                    ));
                }
                writes.push(PreparedSchemaChange::Delete {
                    schema_name: state.schema_name,
                });
            }
            (None, None) => {
                // Delete-on-missing was reported while replaying the edits.
            }
        }
    }

    let prepared = issues.is_empty().then_some(PreparedChange {
        resolved_base_commit: resolved_base_commit.clone(),
        edit_digest: edit_digest.clone(),
        writes,
    });
    Ok(outcome(resolved_base_commit, edit_digest, issues, prepared))
}

pub(crate) fn edit_digest(edits: &[ChangeEdit]) -> CoreResult<String> {
    let canonical = serde_json::to_vec(edits)
        .map_err(|error| CoreError::Other(format!("encode change edits for digest: {error}")))?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(canonical))))
}

fn structural_issues(core: &Core, record: &ChangeRecord) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let limits = TransactionLimits::default();
    if record.edits.is_empty() {
        issues.push(issue(
            "no_edits",
            "change record has no executable edits",
            None,
            None,
        ));
        return issues;
    }
    if record.edits.len() > limits.max_ops {
        issues.push(issue(
            "too_many_edits",
            format!(
                "change has {} edits; the maximum is {}",
                record.edits.len(),
                limits.max_ops
            ),
            None,
            None,
        ));
    }

    let mut formats = BTreeSet::new();
    let mut schemas = BTreeSet::new();
    for edit in &record.edits {
        let (schema, format_id) = edit_target(edit);
        schemas.insert(schema.schema_name.clone());
        formats.insert(format_id.to_string());
        if schema.project != record.project || schema.repo != record.repo {
            issues.push(issue(
                "edit_outside_scope",
                "edit target is outside the change record repository",
                Some(&schema.schema_name),
                None,
            ));
        }
        if schema.schema_name.trim().is_empty() || schema.schema_name.chars().any(char::is_control)
        {
            issues.push(issue(
                "invalid_schema_path",
                "schema path must not be empty or contain control characters",
                Some(&schema.schema_name),
                None,
            ));
        }
        match detect_format_from_name(&schema.schema_name) {
            Some(detected) if detected != format_id => issues.push(issue(
                "format_mismatch",
                format!("schema extension selects format {detected:?}, not {format_id:?}"),
                Some(&schema.schema_name),
                None,
            )),
            None => issues.push(issue(
                "unknown_schema_extension",
                "schema path does not have a supported format extension",
                Some(&schema.schema_name),
                None,
            )),
            Some(_) => {}
        }
        if format_id.trim().is_empty() {
            issues.push(issue(
                "missing_format",
                "edit format_id must not be empty",
                Some(&schema.schema_name),
                None,
            ));
        } else if core.registry.get(format_id).is_none() {
            issues.push(issue(
                "unknown_format",
                format!("no compiler is registered for format {format_id:?}"),
                Some(&schema.schema_name),
                None,
            ));
        }
        if matches!(edit, ChangeEdit::Mutation { operation, .. } if operation.is_empty()) {
            issues.push(issue(
                "empty_mutation",
                "mutation operation bytes must not be empty",
                Some(&schema.schema_name),
                None,
            ));
        }
    }
    if formats.len() > 1 {
        issues.push(issue(
            "mixed_formats",
            "all edits in one change must use the same format",
            None,
            None,
        ));
    }
    if schemas.len() > limits.max_schemas {
        issues.push(issue(
            "too_many_schemas",
            format!(
                "change touches {} schemas; the maximum is {}",
                schemas.len(),
                limits.max_schemas
            ),
            None,
            None,
        ));
    }
    issues
}

fn collect_compatibility_issues(
    compiler: &dyn schemahub_types::Compiler,
    rules: &schemahub_types::CompatibilityRules,
    original: &SchemaObjects,
    effect: &MutationEffect,
    schema_name: &str,
    issues: &mut Vec<ValidationIssue>,
) -> CoreResult<()> {
    match compat::gate(compiler, rules, original, effect) {
        Ok(()) => {}
        Err(CoreError::Incompatible(violations)) => {
            for violation in violations {
                let field_suffix = violation
                    .field_name
                    .as_deref()
                    .map(|field| format!(" (field {field})"))
                    .unwrap_or_default();
                issues.push(issue(
                    "compatibility_violation",
                    format!("{}{}", violation.message, field_suffix),
                    Some(schema_name),
                    Some(&violation.declaration_name),
                ));
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn apply_effect(mut schema: SchemaObjects, effect: MutationEffect) -> SchemaObjects {
    if let Some(meta) = effect.meta {
        schema.meta = meta;
    }
    for (name, blob) in effect.upserts {
        schema.decls.insert(name, blob);
    }
    for name in effect.removes {
        schema.decls.remove(&name);
    }
    schema
}

fn diff_effect(original: &SchemaObjects, current: &SchemaObjects) -> MutationEffect {
    let meta = (original.meta != current.meta).then(|| current.meta.clone());
    let upserts = current
        .decls
        .iter()
        .filter(|(name, blob)| original.decls.get(*name) != Some(*blob))
        .map(|(name, blob)| (name.clone(), blob.clone()))
        .collect();
    let removes = original
        .decls
        .keys()
        .filter(|name| !current.decls.contains_key(*name))
        .cloned()
        .collect();
    MutationEffect {
        meta,
        upserts,
        removes,
    }
}

fn effect_is_empty(effect: &MutationEffect) -> bool {
    effect.meta.is_none() && effect.upserts.is_empty() && effect.removes.is_empty()
}

fn expected_ref_error(error: &JjError) -> bool {
    matches!(
        error,
        JjError::BadRef(_)
            | JjError::ObjectNotFound
            | JjError::BookmarkNotFound(_)
            | JjError::TagNotFound(_)
    )
}

fn edit_target(edit: &ChangeEdit) -> (&schemahub_types::SchemaPath, &str) {
    match edit {
        ChangeEdit::Mutation {
            schema, format_id, ..
        }
        | ChangeEdit::ReplaceSource {
            schema, format_id, ..
        }
        | ChangeEdit::DeleteSchema { schema, format_id } => (schema, format_id),
    }
}

fn edit_format(edits: &[ChangeEdit]) -> Option<&str> {
    edits.first().map(|edit| edit_target(edit).1)
}

fn edit_schema_name(edit: &ChangeEdit) -> &str {
    &edit_target(edit).0.schema_name
}

fn issue(
    code: impl Into<String>,
    message: impl Into<String>,
    schema_name: Option<&str>,
    declaration_name: Option<&str>,
) -> ValidationIssue {
    ValidationIssue {
        code: code.into(),
        message: message.into(),
        schema_name: schema_name.map(str::to_string),
        declaration_name: declaration_name.map(str::to_string),
    }
}

fn outcome(
    resolved_base_commit: String,
    edit_digest: String,
    issues: Vec<ValidationIssue>,
    prepared: Option<PreparedChange>,
) -> ValidationOutcome {
    ValidationOutcome {
        result: ValidationResult {
            valid: issues.is_empty(),
            resolved_base_commit,
            edit_digest,
            issues,
            validated_at_unix_ms: 0,
            validator_version: VALIDATOR_VERSION.to_string(),
        },
        prepared,
    }
}
