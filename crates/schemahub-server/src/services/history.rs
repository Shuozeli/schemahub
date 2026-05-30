//! `HistoryService` — log / op-log / undo / render-conflict / resolve-conflict
//! (design.md §4.4, §6, §12). New in v2 (the v1 protos predate the jj model).

use std::sync::Arc;

use schemahub_core::{detect_format_from_name, Core};
use schemahub_types::{DeclBlob, SchemaPath};
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::history_service_server::HistoryService;

use crate::error::to_status;
use crate::services::token_from;
use crate::wire;

const DEFAULT_AUTHOR: &str = "schemahub";
const DEFAULT_BOOKMARK: &str = "main";

pub struct HistoryHandler {
    core: Arc<Core>,
}

impl HistoryHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl HistoryService for HistoryHandler {
    async fn log(
        &self,
        request: Request<pb::LogRequest>,
    ) -> Result<Response<pb::LogResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let entries = self
            .core
            .log(&r.project, &r.repo, None, None, token.as_deref())
            .map_err(to_status)?;
        let entries = entries
            .into_iter()
            .map(|e| pb::LogEntry {
                commit_id: e.commit_id,
                change_id: e.change_id,
                parents: e.parents,
                author: e.author,
                message: e.message,
                timestamp: e.timestamp,
            })
            .collect();
        Ok(Response::new(pb::LogResponse { entries }))
    }

    async fn op_log(
        &self,
        request: Request<pb::OpLogRequest>,
    ) -> Result<Response<pb::OpLogResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let ops = self
            .core
            .op_log(&r.project, &r.repo, token.as_deref())
            .map_err(to_status)?;
        let operations = ops
            .into_iter()
            .map(|o| pb::OperationRecord {
                op_id: o.op_id,
                parents: o.parents,
                description: o.description,
                author: o.author,
                timestamp: o.timestamp,
            })
            .collect();
        Ok(Response::new(pb::OpLogResponse { operations }))
    }

    async fn undo(
        &self,
        request: Request<pb::UndoRequest>,
    ) -> Result<Response<pb::UndoResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let author = if r.author.is_empty() {
            DEFAULT_AUTHOR
        } else {
            &r.author
        };
        let undone = self
            .core
            .undo(&r.project, &r.repo, author, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::UndoResponse {
            undone_op_id: undone,
        }))
    }

    async fn render_conflict(
        &self,
        request: Request<pb::RenderConflictRequest>,
    ) -> Result<Response<pb::RenderConflictResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        let bookmark = wire::version_ref_bookmark(&r.at, DEFAULT_BOOKMARK);
        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let rendered = self
            .core
            .render_conflict(&schema, &bookmark, &r.declaration_name, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::RenderConflictResponse { rendered }))
    }

    async fn resolve_conflict(
        &self,
        request: Request<pb::ResolveConflictRequest>,
    ) -> Result<Response<pb::ResolveConflictResponse>, Status> {
        let token = token_from(&request);
        let r = request.into_inner();
        // Parse the resolved source and extract the named declaration's blob.
        let format_id = detect_format_from_name(&r.schema_path).ok_or_else(|| {
            Status::invalid_argument(format!("unknown extension: {}", r.schema_path))
        })?;
        let compiler = self
            .core
            .registry()
            .get(format_id)
            .ok_or_else(|| Status::invalid_argument(format!("no compiler for {format_id}")))?;
        let parsed = compiler
            .parse(&r.resolved_source)
            .map_err(|e| Status::invalid_argument(format!("parse error: {e}")))?;
        let resolved: DeclBlob = parsed
            .decls
            .into_iter()
            .find(|(name, _)| name == &r.declaration_name)
            .map(|(_, blob)| blob)
            .ok_or_else(|| {
                Status::invalid_argument(format!(
                    "resolved source does not define '{}'",
                    r.declaration_name
                ))
            })?;

        let schema = SchemaPath::new(&r.project, &r.repo, &r.schema_path);
        let message = if r.message.is_empty() {
            format!("resolve conflict on {}", r.declaration_name)
        } else {
            r.message.clone()
        };
        let author = if r.author.is_empty() {
            DEFAULT_AUTHOR
        } else {
            &r.author
        };
        let resp = self
            .core
            .resolve_conflict(
                &schema,
                &r.bookmark,
                &r.declaration_name,
                resolved,
                author,
                &message,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(pb::ResolveConflictResponse {
            new_commit: resp.commit_id,
            change_id: resp.change_id,
        }))
    }
}
