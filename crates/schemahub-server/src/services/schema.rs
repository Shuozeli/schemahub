//! `SchemaService` — schema lifecycle (create/update/delete) + granular and
//! transactional mutations (design.md §5, crate-structure.md §3.6).
//!
//! Lifecycle handlers are thin wire adapters into the format-agnostic core,
//! which owns parsing, existence, authorization, compatibility, reference
//! integrity, idempotency, and JJ publication policy. Granular mutations route
//! through `Core::apply_mutation` (design.md §5.1 steps 2–10).

use std::sync::Arc;
use std::time::Duration;

use schemahub_core::{
    Core, CreateSchemaRequest as CoreCreateSchemaRequest,
    DeleteSchemaRequest as CoreDeleteSchemaRequest, MutationRequest, TransactionDeadline,
    TransactionRequest, UpdateSchemaRequest as CoreUpdateSchemaRequest,
};
use schemahub_jj::RefSpec;
use schemahub_types::SchemaPath;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::schema_service_server::SchemaService;

use crate::error::to_status;
use crate::services::{resolve_author, token_from};
use crate::wire;

#[derive(Clone)]
pub struct SchemaHandler {
    core: Arc<Core>,
}

impl SchemaHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }

    fn resolve_import_pin(
        &self,
        import_path: &str,
        to_commit: &str,
        to_tag: &str,
        remove: bool,
        token: Option<&str>,
    ) -> Result<Option<String>, Status> {
        if !to_commit.is_empty() && !to_tag.is_empty() {
            return Err(Status::invalid_argument(
                "import update accepts at most one of to_commit and to_tag",
            ));
        }
        if remove {
            if !to_commit.is_empty() || !to_tag.is_empty() {
                return Err(Status::invalid_argument(
                    "import removal must not include a commit or tag pin",
                ));
            }
            return Ok(None);
        }
        let (at, resolved_from) = if !to_commit.is_empty() {
            (RefSpec::commit(to_commit), format!("@{to_commit}"))
        } else if !to_tag.is_empty() {
            (RefSpec::Tag(to_tag.to_string()), format!("tag:{to_tag}"))
        } else {
            return Ok(None);
        };
        let (project, repo, schema_name) = parse_import_path(import_path)?;
        let revision = self
            .core
            .resolve_schema_revision(project, repo, &at, resolved_from, token)
            .map_err(to_status)?;
        self.core
            .jj()
            .load_schema(
                project,
                repo,
                schema_name,
                &RefSpec::commit(&revision.commit_id),
            )
            .map_err(|error| to_status(error.into()))?;
        Ok(Some(revision.commit_id))
    }

    fn normalize_protobuf_import(
        &self,
        mutation: &mut pb::ProtobufMutation,
        token: Option<&str>,
    ) -> Result<(), Status> {
        let Some(pb::protobuf_mutation::Operation::UpdateImport(import)) =
            mutation.operation.as_mut()
        else {
            return Ok(());
        };
        if let Some(commit) = self.resolve_import_pin(
            &import.import_path,
            &import.to_commit,
            &import.to_tag,
            import.remove,
            token,
        )? {
            import.to_commit = commit;
            import.to_tag.clear();
        }
        Ok(())
    }

    fn normalize_flatbuffers_import(
        &self,
        mutation: &mut pb::FlatBuffersMutation,
        token: Option<&str>,
    ) -> Result<(), Status> {
        let Some(pb::flat_buffers_mutation::Operation::UpdateImport(import)) =
            mutation.operation.as_mut()
        else {
            return Ok(());
        };
        if let Some(commit) = self.resolve_import_pin(
            &import.import_path,
            &import.to_commit,
            &import.to_tag,
            import.remove,
            token,
        )? {
            import.to_commit = commit;
            import.to_tag.clear();
        }
        Ok(())
    }

    fn normalize_apply_import(
        &self,
        operation: &mut pb::apply_mutation_request::Operation,
        token: Option<&str>,
    ) -> Result<(), Status> {
        match operation {
            pb::apply_mutation_request::Operation::ProtobufOp(mutation) => {
                self.normalize_protobuf_import(mutation, token)
            }
            pb::apply_mutation_request::Operation::FbsOp(mutation) => {
                self.normalize_flatbuffers_import(mutation, token)
            }
            pb::apply_mutation_request::Operation::OpenapiOp(_) => Ok(()),
        }
    }

    fn normalize_transaction_import(
        &self,
        operation: &mut pb::transaction_op::Operation,
        token: Option<&str>,
    ) -> Result<(), Status> {
        match operation {
            pb::transaction_op::Operation::ProtobufOp(mutation) => {
                self.normalize_protobuf_import(mutation, token)
            }
            pb::transaction_op::Operation::FbsOp(mutation) => {
                self.normalize_flatbuffers_import(mutation, token)
            }
            pb::transaction_op::Operation::OpenapiOp(_) => Ok(()),
        }
    }

    fn apply_transaction_sync(
        &self,
        mut request: pb::ApplyTransactionRequest,
        token: Option<String>,
        deadline: TransactionDeadline,
    ) -> Result<pb::ApplyTransactionResponse, Status> {
        if request.operations.is_empty() {
            return Err(Status::invalid_argument("transaction has no operations"));
        }
        require_transaction_time(&deadline)?;

        let mut mutations = Vec::with_capacity(request.operations.len());
        for tx_op in &mut request.operations {
            require_transaction_time(&deadline)?;
            let op = tx_op
                .operation
                .as_mut()
                .ok_or_else(|| Status::invalid_argument("transaction op oneof not set"))?;
            self.normalize_transaction_import(op, token.as_deref())?;
            mutations.push(wire::transaction_op_to_core(
                &request.project,
                &request.repo,
                op,
            )?);
            require_transaction_time(&deadline)?;
        }

        let author = resolve_author(&self.core, token.as_deref())?;
        require_transaction_time(&deadline)?;
        let core_request = TransactionRequest {
            bookmark: request.branch.clone(),
            mutations,
            author,
            message: format!("transaction on {}", request.branch),
            force: request.force,
            idempotency_key: (!request.idempotency_key.is_empty())
                .then(|| request.idempotency_key.clone()),
            base_revision: (!request.base_revision.is_empty())
                .then(|| request.base_revision.clone()),
            token,
        };
        let response = self
            .core
            .apply_mutations_with_deadline(core_request, deadline)
            .map_err(to_status)?;
        Ok(pb::ApplyTransactionResponse {
            new_commit: response.commit_id,
            change_id: response.change_id,
            conflicted_decls: response.conflicted_decls,
        })
    }
}

fn transaction_deadline_status() -> Status {
    Status::deadline_exceeded(format!(
        "transaction exceeded the server execution deadline of {} seconds",
        crate::TRANSACTION_TIMEOUT_SECS
    ))
}

fn require_transaction_time(deadline: &TransactionDeadline) -> Result<(), Status> {
    if deadline.is_exceeded() {
        Err(transaction_deadline_status())
    } else {
        Ok(())
    }
}

async fn await_transaction<T>(
    deadline: TransactionDeadline,
    mut task: tokio::task::JoinHandle<Result<T, Status>>,
) -> Result<T, Status>
where
    T: Send + 'static,
{
    let timer = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.expires_at()));
    tokio::pin!(timer);
    tokio::select! {
        biased;
        result = &mut task => match result {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(
                    event = "schemahub.transaction.worker_failed",
                    error = %error,
                );
                Err(Status::internal("transaction execution worker failed"))
            }
        },
        _ = &mut timer => {
            deadline.cancel();
            task.abort();
            tracing::warn!(
                event = "schemahub.transaction.deadline_exceeded",
                timeout_secs = crate::TRANSACTION_TIMEOUT_SECS,
            );
            Err(transaction_deadline_status())
        }
    }
}

fn parse_import_path(import_path: &str) -> Result<(&str, &str, &str), Status> {
    let mut parts = import_path.splitn(3, '/');
    let project = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    let schema_name = parts.next().unwrap_or_default();
    if project.is_empty() || repo.is_empty() || schema_name.is_empty() {
        return Err(Status::invalid_argument(
            "a pinned import_path must be project/repo/schema-file",
        ));
    }
    Ok((project, repo, schema_name))
}

#[tonic::async_trait]
impl SchemaService for SchemaHandler {
    async fn create_schema(
        &self,
        request: Request<pb::CreateSchemaRequest>,
    ) -> Result<Response<pb::CreateSchemaResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let author = resolve_author(&self.core, token.as_deref())?;
        let message = format!("create schema {}", r.schema_name);
        let format = pb::SchemaFormat::try_from(r.format).map_err(|_| {
            Status::invalid_argument(format!("unknown schema format: {}", r.format))
        })?;
        let format_id = wire::format_id_from_pb(format)
            .ok_or_else(|| Status::invalid_argument("schema format must be specified"))?;
        let write = self
            .core
            .create_schema(CoreCreateSchemaRequest {
                schema: SchemaPath::new(r.project, r.repo, r.schema_name),
                bookmark: r.branch,
                format_id: format_id.to_string(),
                source: r.source,
                author,
                message,
                idempotency_key: (!r.idempotency_key.is_empty()).then_some(r.idempotency_key),
                base_revision: (!r.base_revision.is_empty()).then_some(r.base_revision),
                token,
            })
            .map_err(to_status)?;
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
        let token = token_from(&request)?;
        let r = request.into_inner();
        let author = resolve_author(&self.core, token.as_deref())?;
        let message = format!("update schema {}", r.schema_name);
        let write = self
            .core
            .update_schema(CoreUpdateSchemaRequest {
                schema: SchemaPath::new(r.project, r.repo, r.schema_name),
                bookmark: r.branch,
                source: r.source,
                author,
                message,
                force: r.force,
                idempotency_key: (!r.idempotency_key.is_empty()).then_some(r.idempotency_key),
                base_revision: (!r.base_revision.is_empty()).then_some(r.base_revision),
                token,
            })
            .map_err(to_status)?;
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
        let token = token_from(&request)?;
        let r = request.into_inner();
        let author = resolve_author(&self.core, token.as_deref())?;
        let message = format!("delete schema {}", r.schema_name);
        let write = self
            .core
            .delete_schema(CoreDeleteSchemaRequest {
                schema: SchemaPath::new(r.project, r.repo, r.schema_name),
                bookmark: r.branch,
                author,
                message,
                force: r.force,
                idempotency_key: (!r.idempotency_key.is_empty()).then_some(r.idempotency_key),
                base_revision: (!r.base_revision.is_empty()).then_some(r.base_revision),
                token,
            })
            .map_err(to_status)?;
        Ok(Response::new(pb::DeleteSchemaResponse {
            new_commit: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        }))
    }

    async fn apply_mutation(
        &self,
        request: Request<pb::ApplyMutationRequest>,
    ) -> Result<Response<pb::ApplyMutationResponse>, Status> {
        let token = token_from(&request)?;
        let mut r = request.into_inner();
        let op = r
            .operation
            .as_mut()
            .ok_or_else(|| Status::invalid_argument("apply_mutation: operation oneof not set"))?;
        self.normalize_apply_import(op, token.as_deref())?;
        let mutation = wire::apply_mutation_op_to_core(&r.project, &r.repo, op)?;
        let author = resolve_author(&self.core, token.as_deref())?;
        let req = MutationRequest {
            bookmark: r.branch.clone(),
            mutation,
            author,
            message: format!("mutation on {}", r.branch),
            force: r.force,
            idempotency_key: (!r.idempotency_key.is_empty()).then(|| r.idempotency_key.clone()),
            base_revision: (!r.base_revision.is_empty()).then(|| r.base_revision.clone()),
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
        let token = token_from(&request)?;
        let deadline =
            TransactionDeadline::after(Duration::from_secs(crate::TRANSACTION_TIMEOUT_SECS));
        let worker_deadline = deadline.clone();
        let handler = self.clone();
        let request = request.into_inner();
        let task = tokio::task::spawn_blocking(move || {
            handler.apply_transaction_sync(request, token, worker_deadline)
        });
        await_transaction(deadline, task).await.map(Response::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn server_deadline_returns_deadline_exceeded_for_running_transaction() {
        // Arrange: keep a blocking worker alive until after the short server
        // timer fires, then release it so the test leaves no detached work.
        let deadline = TransactionDeadline::after(Duration::from_millis(10));
        let observed_deadline = deadline.clone();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let task = tokio::task::spawn_blocking(move || {
            release_rx.recv().expect("release transaction worker");
            Ok::<_, Status>(())
        });

        // Act
        let result = await_transaction(deadline, task).await;
        release_tx.send(()).expect("stop transaction worker");

        // Assert
        assert_eq!(
            result.expect_err("deadline should reject").code(),
            tonic::Code::DeadlineExceeded
        );
        assert!(observed_deadline.is_exceeded());
    }
}
