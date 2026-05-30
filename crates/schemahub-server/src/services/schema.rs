//! `SchemaService` — schema lifecycle (create/update/delete) + granular and
//! transactional mutations (design.md §5, crate-structure.md §3.6).
//!
//! Lifecycle (create/update/delete-from-source) is format-agnostic: the server
//! parses `source` with the compiler selected by file extension, turns the
//! resulting [`ParsedSchema`] into a [`MutationEffect`], and commits via the
//! VCS. Granular mutations route through `Core::apply_mutation`, which runs the
//! auth + compatibility gate (design.md §5.1 steps 2–10).

use std::sync::Arc;

use schemahub_core::{detect_format_from_name, Core, MutationRequest, TransactionRequest};
use schemahub_types::{Action, MutationEffect, SchemaObjects};
use schemahub_vcs::RefSpec;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::schema_service_server::SchemaService;

use crate::error::to_status;
use crate::services::token_from;
use crate::wire;

/// The default commit author when no auth identity is resolved.
const DEFAULT_AUTHOR: &str = "schemahub";

pub struct SchemaHandler {
    core: Arc<Core>,
}

impl SchemaHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }

    /// Parse `source` into the per-declaration objects and build a
    /// [`MutationEffect`] that replaces the schema file's whole content (meta +
    /// every decl), removing any decls present in `base` that the new source
    /// dropped.
    fn effect_from_source(
        &self,
        schema_name: &str,
        source: &str,
        base: &SchemaObjects,
    ) -> Result<MutationEffect, Status> {
        let format_id = detect_format_from_name(schema_name)
            .ok_or_else(|| Status::invalid_argument(format!("unknown extension: {schema_name}")))?;
        let compiler = self
            .core
            .registry()
            .get(format_id)
            .ok_or_else(|| Status::invalid_argument(format!("no compiler for {format_id}")))?;
        let parsed = compiler
            .parse(source)
            .map_err(|e| Status::invalid_argument(format!("parse error: {e}")))?;

        let new_names: std::collections::BTreeSet<&String> =
            parsed.decls.iter().map(|(n, _)| n).collect();
        let removes: Vec<String> = base
            .decls
            .keys()
            .filter(|n| !new_names.contains(n))
            .cloned()
            .collect();

        Ok(MutationEffect {
            meta: Some(parsed.meta),
            upserts: parsed.decls,
            removes,
        })
    }
}

#[tonic::async_trait]
impl SchemaService for SchemaHandler {
    async fn create_schema(
        &self,
        request: Request<pb::CreateSchemaRequest>,
    ) -> Result<Response<pb::CreateSchemaResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        // Auth gate: schema lifecycle is a Write on (project, repo).
        self.core
            .authorize_repo_action(token.as_deref(), Action::Write, &r.project, &r.repo)
            .map_err(to_status)?;
        let base_ref = RefSpec::bookmark(r.branch.clone());
        // First write may target a fresh bookmark; tolerate a missing base.
        let base = self
            .core
            .vcs()
            .load_schema(&r.project, &r.repo, &r.schema_name, &base_ref)
            .unwrap_or_default();
        let effect = self.effect_from_source(&r.schema_name, &r.source, &base)?;
        let write = self
            .core
            .vcs()
            .commit_write(
                &r.project,
                &r.repo,
                &r.branch,
                &r.schema_name,
                &base_ref,
                effect,
                DEFAULT_AUTHOR,
                &format!("create schema {}", r.schema_name),
            )
            .map_err(|e| to_status(e.into()))?;
        Ok(Response::new(pb::CreateSchemaResponse {
            new_commit: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        }))
    }

    async fn update_schema(
        &self,
        request: Request<pb::UpdateSchemaRequest>,
    ) -> Result<Response<pb::UpdateSchemaResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        self.core
            .authorize_repo_action(token.as_deref(), Action::Write, &r.project, &r.repo)
            .map_err(to_status)?;
        let base_ref = RefSpec::bookmark(r.branch.clone());
        let base = self
            .core
            .vcs()
            .load_schema(&r.project, &r.repo, &r.schema_name, &base_ref)
            .unwrap_or_default();
        let effect = self.effect_from_source(&r.schema_name, &r.source, &base)?;
        let write = self
            .core
            .vcs()
            .commit_write(
                &r.project,
                &r.repo,
                &r.branch,
                &r.schema_name,
                &base_ref,
                effect,
                DEFAULT_AUTHOR,
                &format!("update schema {}", r.schema_name),
            )
            .map_err(|e| to_status(e.into()))?;
        Ok(Response::new(pb::UpdateSchemaResponse {
            new_commit: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        }))
    }

    async fn delete_schema(
        &self,
        request: Request<pb::DeleteSchemaRequest>,
    ) -> Result<Response<pb::DeleteSchemaResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        // Deleting a schema file is a Write — design.md §6 protected-bookmark
        // policy is enforced by the VCS layer; auth gates the repo overall.
        self.core
            .authorize_repo_action(token.as_deref(), Action::Write, &r.project, &r.repo)
            .map_err(to_status)?;
        let base_ref = RefSpec::bookmark(r.branch.clone());
        let base = self
            .core
            .vcs()
            .load_schema(&r.project, &r.repo, &r.schema_name, &base_ref)
            .map_err(|e| to_status(e.into()))?;
        // Remove every declaration in the file (empties the subtree).
        let effect = MutationEffect {
            meta: None,
            upserts: vec![],
            removes: base.decls.keys().cloned().collect(),
        };
        let write = self
            .core
            .vcs()
            .commit_write(
                &r.project,
                &r.repo,
                &r.branch,
                &r.schema_name,
                &base_ref,
                effect,
                DEFAULT_AUTHOR,
                &format!("delete schema {}", r.schema_name),
            )
            .map_err(|e| to_status(e.into()))?;
        Ok(Response::new(pb::DeleteSchemaResponse {
            new_commit: write.commit_id,
            change_id: write.change_id,
        }))
    }

    async fn apply_mutation(
        &self,
        request: Request<pb::ApplyMutationRequest>,
    ) -> Result<Response<pb::ApplyMutationResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let op = r
            .operation
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("apply_mutation: operation oneof not set"))?;
        let mutation = wire::apply_mutation_op_to_core(&r.project, &r.repo, op)?;
        let req = MutationRequest {
            bookmark: r.branch.clone(),
            mutation,
            author: DEFAULT_AUTHOR.to_string(),
            message: format!("mutation on {}", r.branch),
            force: r.force,
            idempotency_key: (!r.idempotency_key.is_empty()).then(|| r.idempotency_key.clone()),
            token,
        };
        let resp = self.core.apply_mutation(req).map_err(to_status)?;
        Ok(Response::new(pb::ApplyMutationResponse {
            new_commit: resp.commit_id,
            change_id: resp.change_id,
            conflicted_decls: resp.conflicted_decls,
        }))
    }

    async fn apply_transaction(
        &self,
        request: Request<pb::ApplyTransactionRequest>,
    ) -> Result<Response<pb::ApplyTransactionResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        if r.operations.is_empty() {
            return Err(Status::invalid_argument("transaction has no operations"));
        }
        let mut mutations = Vec::with_capacity(r.operations.len());
        for tx_op in &r.operations {
            let op = tx_op
                .operation
                .as_ref()
                .ok_or_else(|| Status::invalid_argument("transaction op oneof not set"))?;
            mutations.push(wire::transaction_op_to_core(&r.project, &r.repo, op)?);
        }
        let req = TransactionRequest {
            bookmark: r.branch.clone(),
            mutations,
            author: DEFAULT_AUTHOR.to_string(),
            message: format!("transaction on {}", r.branch),
            force: r.force,
            idempotency_key: (!r.idempotency_key.is_empty()).then(|| r.idempotency_key.clone()),
            token,
        };
        let resp = self.core.apply_mutations(req).map_err(to_status)?;
        Ok(Response::new(pb::ApplyTransactionResponse {
            new_commit: resp.commit_id,
            change_id: resp.change_id,
            conflicted_decls: resp.conflicted_decls,
        }))
    }
}
