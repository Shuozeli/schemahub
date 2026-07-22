//! Whole-schema lifecycle orchestration.
//!
//! Transport adapters supply plain request values; this module owns format,
//! existence, authorization, compatibility, reference-integrity, immutable
//! planning-base, and durable-idempotency policy before publishing one JJ
//! operation.

use std::collections::{BTreeMap, BTreeSet};

use schemahub_jj::{JjError, RefSpec, SchemaWrite};
use schemahub_types::{Action, CompatibilityViolation, Compiler, MutationEffect, SchemaObjects};

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::idempotency::{force_audit_attributes, FingerprintBuilder};
use crate::mutation::{compat, immutable_bookmark_base};
use crate::request::{
    CreateSchemaRequest, DeleteSchemaRequest, MutationResponse, UpdateSchemaRequest,
};
use crate::{detect_format_from_name, Core};

impl Core {
    /// Create a schema that does not exist at the bookmark snapshot.
    pub fn create_schema(&self, req: CreateSchemaRequest) -> CoreResult<MutationResponse> {
        let path = &req.schema;
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            req.token.as_deref(),
            Action::Write,
            &path.project,
            &path.repo,
        )?;
        let (scope, fingerprint) = lifecycle_contract(
            "create-schema",
            &req.schema,
            &req.bookmark,
            req.base_revision.as_deref(),
            Some(&req.format_id),
            Some(&req.source),
            false,
            &req.author,
            &req.message,
        );
        if let Some(response) = self.replay_idempotent_write(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &path.project,
            &path.repo,
            &req.bookmark,
        )? {
            return Ok(response);
        }
        self.validate_base_revision(&path.project, &path.repo, req.base_revision.as_deref())?;
        self.ensure_direct_write_allowed(&path.project, &path.repo)?;
        let compiler = self.compiler_for_explicit_format(&path.schema_name, &req.format_id)?;
        let base_ref = immutable_bookmark_base(&self.jj, &path.project, &path.repo, &req.bookmark)?;
        match self
            .jj
            .load_schema(&path.project, &path.repo, &path.schema_name, &base_ref)
        {
            Ok(_) => {
                return Err(CoreError::AlreadyExists(format!(
                    "schema {path} already exists on bookmark {:?}",
                    req.bookmark
                )))
            }
            Err(JjError::SchemaNotFound(_)) => {}
            Err(error) => return Err(error.into()),
        }
        let effect = replacement_effect(compiler.as_ref(), &req.source, &SchemaObjects::default())?;

        self.commit_idempotent_schema_changes_with_attributes(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &path.project,
            &path.repo,
            &req.bookmark,
            &base_ref,
            vec![SchemaWrite::Patch {
                schema_path: path.schema_name.clone(),
                effect,
            }],
            &req.author,
            &req.message,
            force_audit_attributes(false),
        )
    }

    /// Replace an existing schema, enforcing compatibility on protected
    /// bookmarks unless an authorized caller explicitly forces the write.
    pub fn update_schema(&self, req: UpdateSchemaRequest) -> CoreResult<MutationResponse> {
        let path = &req.schema;
        let action = if req.force {
            Action::Force
        } else {
            Action::Write
        };
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            req.token.as_deref(),
            action,
            &path.project,
            &path.repo,
        )?;
        let (scope, fingerprint) = lifecycle_contract(
            "update-schema",
            &req.schema,
            &req.bookmark,
            req.base_revision.as_deref(),
            None,
            Some(&req.source),
            req.force,
            &req.author,
            &req.message,
        );
        if let Some(response) = self.replay_idempotent_write(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &path.project,
            &path.repo,
            &req.bookmark,
        )? {
            return Ok(response);
        }
        self.validate_base_revision(&path.project, &path.repo, req.base_revision.as_deref())?;
        self.ensure_direct_write_allowed(&path.project, &path.repo)?;
        let compiler = self.compiler_for(&path.schema_name)?;
        let base_ref = immutable_bookmark_base(&self.jj, &path.project, &path.repo, &req.bookmark)?;
        let base = self
            .jj
            .load_schema(&path.project, &path.repo, &path.schema_name, &base_ref)?;
        let effect = replacement_effect(compiler.as_ref(), &req.source, &base)?;
        let config = self.effective_repo_config(&path.project, &path.repo)?;
        if !req.force
            && schemahub_jj::bookmark::is_protected(&req.bookmark, &config.protected_bookmarks)
        {
            compat::gate(compiler.as_ref(), &config.compat_rules(), &base, &effect)?;
        }

        self.commit_idempotent_schema_changes_with_attributes(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &path.project,
            &path.repo,
            &req.bookmark,
            &base_ref,
            vec![SchemaWrite::Patch {
                schema_path: path.schema_name.clone(),
                effect,
            }],
            &req.author,
            &req.message,
            force_audit_attributes(req.force),
        )
    }

    /// Delete an existing schema. Force may bypass compatibility policy but
    /// never permits a dangling live import.
    pub fn delete_schema(&self, req: DeleteSchemaRequest) -> CoreResult<MutationResponse> {
        let path = &req.schema;
        let action = if req.force {
            Action::Force
        } else {
            Action::Write
        };
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            req.token.as_deref(),
            action,
            &path.project,
            &path.repo,
        )?;
        let (scope, fingerprint) = lifecycle_contract(
            "delete-schema",
            &req.schema,
            &req.bookmark,
            req.base_revision.as_deref(),
            None,
            None,
            req.force,
            &req.author,
            &req.message,
        );
        if let Some(response) = self.replay_idempotent_write(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &path.project,
            &path.repo,
            &req.bookmark,
        )? {
            return Ok(response);
        }
        self.validate_base_revision(&path.project, &path.repo, req.base_revision.as_deref())?;
        self.ensure_direct_write_allowed(&path.project, &path.repo)?;
        let base_ref = immutable_bookmark_base(&self.jj, &path.project, &path.repo, &req.bookmark)?;
        let base = self
            .jj
            .load_schema(&path.project, &path.repo, &path.schema_name, &base_ref)?;
        self.compiler_for(&path.schema_name)?;
        self.ensure_no_live_dependents(path, &base_ref)?;

        let config = self.effective_repo_config(&path.project, &path.repo)?;
        if !req.force
            && !config.compat_rules().disabled
            && schemahub_jj::bookmark::is_protected(&req.bookmark, &config.protected_bookmarks)
        {
            return Err(CoreError::Incompatible(vec![CompatibilityViolation {
                declaration_name: path.schema_name.clone(),
                field_name: None,
                message: "deleting an entire schema is blocked on a protected bookmark".to_string(),
            }]));
        }
        let _ = base;

        self.commit_idempotent_schema_changes_with_attributes(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &path.project,
            &path.repo,
            &req.bookmark,
            &base_ref,
            vec![SchemaWrite::Delete {
                schema_path: path.schema_name.clone(),
            }],
            &req.author,
            &req.message,
            force_audit_attributes(req.force),
        )
    }

    fn compiler_for_explicit_format(
        &self,
        schema_name: &str,
        format_id: &str,
    ) -> CoreResult<std::sync::Arc<dyn Compiler>> {
        let detected = detect_format_from_name(schema_name)
            .ok_or_else(|| CoreError::UndetectableFormat(schema_name.to_string()))?;
        if format_id.is_empty() {
            return Err(CoreError::InvalidArgument(
                "schema format must be specified".to_string(),
            ));
        }
        if detected != format_id {
            return Err(CoreError::InvalidArgument(format!(
                "schema extension selects format {detected:?}, not {format_id:?}"
            )));
        }
        self.registry
            .get(format_id)
            .cloned()
            .ok_or_else(|| CoreError::UnknownFormat(format_id.to_string()))
    }

    fn ensure_no_live_dependents(
        &self,
        target: &schemahub_types::SchemaPath,
        at: &RefSpec,
    ) -> CoreResult<()> {
        let mut dependents = self.live_unpinned_dependents(target, at, &BTreeMap::new())?;
        if dependents.is_empty() {
            return Ok(());
        }
        dependents.sort();
        Err(CoreError::FailedPrecondition(format!(
            "schema {target} has live unpinned dependents: {}; update or remove those imports before deleting it",
            dependents.join(", ")
        )))
    }
}

fn replacement_effect(
    compiler: &dyn Compiler,
    source: &str,
    base: &SchemaObjects,
) -> CoreResult<MutationEffect> {
    let parsed = compiler.parse(source)?;
    let mut names = BTreeSet::new();
    for (name, _) in &parsed.decls {
        if !names.insert(name.clone()) {
            return Err(CoreError::InvalidArgument(format!(
                "parsed source contains duplicate declaration {name:?}"
            )));
        }
    }
    let removes = base
        .decls
        .keys()
        .filter(|name| !names.contains(*name))
        .cloned()
        .collect();
    Ok(MutationEffect {
        meta: Some(parsed.meta),
        upserts: parsed.decls,
        removes,
    })
}

#[allow(clippy::too_many_arguments)]
fn lifecycle_contract(
    kind: &str,
    schema: &schemahub_types::SchemaPath,
    bookmark: &str,
    base_revision: Option<&str>,
    format_id: Option<&str>,
    source: Option<&str>,
    force: bool,
    author: &str,
    message: &str,
) -> (String, String) {
    let mut fingerprint = FingerprintBuilder::new(kind);
    for field in [
        schema.project.as_bytes(),
        schema.repo.as_bytes(),
        bookmark.as_bytes(),
        schema.schema_name.as_bytes(),
        base_revision.unwrap_or_default().as_bytes(),
        format_id.unwrap_or_default().as_bytes(),
        source.unwrap_or_default().as_bytes(),
        author.as_bytes(),
        message.as_bytes(),
    ] {
        fingerprint.update(field);
    }
    fingerprint.update(&[u8::from(force)]);
    (
        format!("{kind}/{}/{}", schema.project, schema.repo),
        fingerprint.finish(),
    )
}
