//! `RefService` — commits, diff, branches (== bookmarks), tags, merge
//! (design.md §6, §12). Branch RPCs are kept working over the jj bookmark model
//! (branch name == bookmark name). Commit graph reads walk the real
//! commit/change graph via `Core::log` (`Jj::commit_log`).

use std::pin::Pin;
use std::sync::Arc;

use schemahub_core::Core;
use schemahub_jj::RefSpec;
use schemahub_types::{Action, DeclChange};
use tokio_stream::Stream;
use tonic::metadata::MetadataValue;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::ref_service_server::RefService;

use crate::error::to_status;
use crate::services::{refspec_or_repository_default, resolve_author, token_from};

const MAX_COMMIT_STREAM_SCAN: usize = 10_000;

pub struct BookmarkHandler {
    core: Arc<Core>,
}

impl BookmarkHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl RefService for BookmarkHandler {
    // ── Commits ───────────────────────────────────────────────────────────────

    async fn get_commit(
        &self,
        request: Request<pb::GetCommitRequest>,
    ) -> Result<Response<pb::GetCommitResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        // Look up the commit in the real commit/change graph by its hash.
        let at = RefSpec::commit(r.commit.clone());
        let entries = self
            .core
            .log(&r.project, &r.repo, Some(&at), Some(1), token.as_deref())
            .map_err(to_status)?;
        let entry = entries
            .into_iter()
            .find(|e| e.commit_id == r.commit)
            .ok_or_else(|| Status::not_found(format!("commit {} not found", r.commit)))?;
        Ok(Response::new(pb::GetCommitResponse {
            commit: Some(log_entry_to_commit_info(&entry)),
        }))
    }

    type ListCommitsStream =
        Pin<Box<dyn Stream<Item = Result<pb::CommitInfo, Status>> + Send + 'static>>;

    async fn list_commits(
        &self,
        request: Request<pb::ListCommitsRequest>,
    ) -> Result<Response<Self::ListCommitsStream>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        // Walk the real commit graph from `from` (default: default branch HEAD),
        // already returned newest-first by the JJ layer.
        let from = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.from,
            Action::Read,
            token.as_deref(),
        )?;
        let (entries, at_commit) = self
            .core
            .list_commits_resolved(
                &r.project,
                &r.repo,
                Some(&from),
                (!r.stop_at_commit.is_empty()).then_some(r.stop_at_commit.as_str()),
                (!r.schema_path.is_empty()).then_some(r.schema_path.as_str()),
                MAX_COMMIT_STREAM_SCAN,
                token.as_deref(),
            )
            .map_err(to_status)?;
        let stream: Self::ListCommitsStream = Box::pin(tokio_stream::iter(
            entries
                .into_iter()
                .map(|e| Ok(log_entry_to_commit_info(&e))),
        ));
        let mut response = Response::new(stream);
        response.metadata_mut().insert(
            "x-schemahub-at-commit",
            MetadataValue::try_from(at_commit.as_str())
                .map_err(|_| Status::internal("resolved commit is not valid response metadata"))?,
        );
        Ok(response)
    }

    async fn diff(
        &self,
        request: Request<pb::DiffRequest>,
    ) -> Result<Response<pb::DiffResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let base = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.base,
            Action::Read,
            token.as_deref(),
        )?;
        let head = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.head,
            Action::Read,
            token.as_deref(),
        )?;
        let schema_name = (!r.schema_path.is_empty()).then_some(r.schema_path.as_str());
        let diff = self
            .core
            .diff_repository_resolved(
                &r.project,
                &r.repo,
                schema_name,
                &base,
                &head,
                token.as_deref(),
            )
            .map_err(to_status)?;
        let schema_diffs = diff
            .schema_diffs
            .into_iter()
            .map(|(schema_path, changes)| pb::SchemaDiff {
                schema_path,
                changes: changes.iter().map(decl_change_to_pb).collect(),
            })
            .collect();
        Ok(Response::new(pb::DiffResponse {
            schema_diffs,
            base_commit: diff.base_commit,
            head_commit: diff.head_commit,
        }))
    }

    // ── Branches (== bookmarks) ─────────────────────────────────────────────────

    async fn create_branch(
        &self,
        request: Request<pb::CreateBranchRequest>,
    ) -> Result<Response<pb::CreateBranchResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let from = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.from,
            Action::Write,
            token.as_deref(),
        )?;
        let author = resolve_author(&self.core, token.as_deref())?;
        let head = self
            .core
            .create_bookmark(
                &r.project,
                &r.repo,
                &r.name,
                &from,
                &author,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(pb::CreateBranchResponse {
            branch: Some(pb::BranchInfo {
                project: r.project,
                repo: r.repo,
                name: r.name,
                head_commit: head,
                protected: false,
            }),
        }))
    }

    async fn delete_branch(
        &self,
        request: Request<pb::DeleteBranchRequest>,
    ) -> Result<Response<pb::DeleteBranchResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let author = resolve_author(&self.core, token.as_deref())?;
        self.core
            .delete_bookmark(&r.project, &r.repo, &r.name, &author, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::DeleteBranchResponse {}))
    }

    async fn list_branches(
        &self,
        request: Request<pb::ListBranchesRequest>,
    ) -> Result<Response<pb::ListBranchesResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let bms = self
            .core
            .list_bookmarks(&r.project, &r.repo, token.as_deref())
            .map_err(to_status)?;
        let branches = bms
            .into_iter()
            .filter(|(name, _)| r.name_prefix.is_empty() || name.starts_with(&r.name_prefix))
            .map(|(name, targets)| pb::BranchInfo {
                project: r.project.clone(),
                repo: r.repo.clone(),
                name,
                head_commit: targets.first().cloned().unwrap_or_default(),
                protected: false,
            })
            .collect();
        Ok(Response::new(pb::ListBranchesResponse { branches }))
    }

    async fn get_branch(
        &self,
        request: Request<pb::GetBranchRequest>,
    ) -> Result<Response<pb::GetBranchResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let bms = self
            .core
            .list_bookmarks(&r.project, &r.repo, token.as_deref())
            .map_err(to_status)?;
        let (name, targets) = bms
            .into_iter()
            .find(|(name, _)| name == &r.name)
            .ok_or_else(|| Status::not_found(format!("branch {} not found", r.name)))?;
        Ok(Response::new(pb::GetBranchResponse {
            branch: Some(pb::BranchInfo {
                project: r.project,
                repo: r.repo,
                name,
                head_commit: targets.first().cloned().unwrap_or_default(),
                protected: false,
            }),
        }))
    }

    // ── Tags ─────────────────────────────────────────────────────────────────

    async fn create_tag(
        &self,
        request: Request<pb::CreateTagRequest>,
    ) -> Result<Response<pb::CreateTagResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let at = refspec_or_repository_default(
            &self.core,
            &r.project,
            &r.repo,
            &r.target,
            Action::Write,
            token.as_deref(),
        )?;
        let author = resolve_author(&self.core, token.as_deref())?;
        let commit = self
            .core
            .create_tag(&r.project, &r.repo, &r.name, &at, &author, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::CreateTagResponse {
            tag: Some(pb::TagInfo {
                project: r.project,
                repo: r.repo,
                name: r.name,
                commit_hash: commit,
                annotated: !r.message.is_empty(),
                tagger: if r.message.is_empty() {
                    String::new()
                } else {
                    author.clone()
                },
                message: r.message,
                timestamp: None,
            }),
        }))
    }

    async fn delete_tag(
        &self,
        request: Request<pb::DeleteTagRequest>,
    ) -> Result<Response<pb::DeleteTagResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        // Tags are immutable pins; deletion requires an explicit force flag
        // (proto contract: FAILED_PRECONDITION otherwise).
        if !r.force {
            return Err(Status::failed_precondition(
                "deleting a tag requires force=true (tags are immutable)",
            ));
        }
        let author = resolve_author(&self.core, token.as_deref())?;
        self.core
            .delete_tag(&r.project, &r.repo, &r.name, &author, token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::DeleteTagResponse {}))
    }

    async fn list_tags(
        &self,
        request: Request<pb::ListTagsRequest>,
    ) -> Result<Response<pb::ListTagsResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let tags = self
            .core
            .list_tags(&r.project, &r.repo, token.as_deref())
            .map_err(to_status)?;
        let tags = tags
            .into_iter()
            .filter(|(name, _)| r.name_prefix.is_empty() || name.starts_with(&r.name_prefix))
            .map(|(name, commit)| pb::TagInfo {
                project: r.project.clone(),
                repo: r.repo.clone(),
                name,
                commit_hash: commit,
                annotated: false,
                tagger: String::new(),
                message: String::new(),
                timestamp: None,
            })
            .collect();
        Ok(Response::new(pb::ListTagsResponse { tags }))
    }

    // ── Merge ─────────────────────────────────────────────────────────────────

    async fn merge(
        &self,
        request: Request<pb::MergeRequest>,
    ) -> Result<Response<pb::MergeResponse>, Status> {
        let token = token_from(&request)?;
        let r = request.into_inner();
        let author = resolve_author(&self.core, token.as_deref())?;
        let resp = self
            .core
            .merge_idempotent(
                &r.project,
                &r.repo,
                &r.source_branch,
                &r.target_branch,
                (!r.base_revision.is_empty()).then_some(r.base_revision.as_str()),
                (!r.idempotency_key.is_empty()).then_some(r.idempotency_key.as_str()),
                (!r.message.is_empty()).then_some(r.message.as_str()),
                &author,
                token.as_deref(),
            )
            .map_err(to_status)?;
        Ok(Response::new(pb::MergeResponse {
            new_commit: resp.commit_id,
        }))
    }
}

fn log_entry_to_commit_info(e: &schemahub_core::LogEntry) -> pb::CommitInfo {
    pb::CommitInfo {
        hash: e.commit_id.clone(),
        parent_hashes: e.parents.clone(),
        timestamp: None,
        author: e.author.clone(),
        message: e.message.clone(),
        force: false,
        format_id: String::new(),
    }
}

fn decl_change_to_pb(c: &DeclChange) -> pb::DeclarationChange {
    let (change_type, decl_name, detail) = match c {
        DeclChange::DeclarationAdded { name } => ("added", name.clone(), Vec::new()),
        DeclChange::DeclarationRemoved { name } => ("removed", name.clone(), Vec::new()),
        DeclChange::DeclarationModified { name, detail } => {
            ("modified", name.clone(), detail.to_vec())
        }
    };
    pb::DeclarationChange {
        change_type: change_type.to_string(),
        decl_name,
        detail,
    }
}
