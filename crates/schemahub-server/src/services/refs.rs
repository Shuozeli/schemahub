use std::sync::Arc;

use prost_types::Timestamp;
use schemahub_core::Core;
use schemahub_types::Hash;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    BranchInfo, CommitInfo, CreateBranchRequest, CreateBranchResponse, CreateTagRequest,
    CreateTagResponse, DeleteBranchRequest, DeleteBranchResponse, DeleteTagRequest,
    DeleteTagResponse, DiffRequest, DiffResponse, GetBranchRequest, GetBranchResponse,
    GetCommitRequest, GetCommitResponse, ListBranchesRequest, ListBranchesResponse,
    ListCommitsRequest, ListTagsRequest, ListTagsResponse, MergeRequest, MergeResponse, TagInfo,
    ref_service_server::RefService,
    version_ref::Ref as VersionRefKind,
};

use crate::error::core_to_status;

pub struct RefServiceImpl {
    core: Arc<Core>,
}

impl RefServiceImpl {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

/// A never-yielding stream pinned behind a Box.
pub type BoxStream<T> =
    std::pin::Pin<Box<dyn tonic::codegen::tokio_stream::Stream<Item = Result<T, Status>> + Send>>;

/// Resolve a proto VersionRef to a commit Hash.
fn resolve_version_ref(
    core: &Core,
    project: &str,
    repo: &str,
    vref: Option<schemahub_api::schemahub_v1::VersionRef>,
) -> Result<Hash, Status> {
    match vref {
        Some(v) => match v.r#ref {
            Some(VersionRefKind::Branch(branch)) => {
                core.get_branch_head(project, repo, &branch)
                    .map_err(core_to_status)
            }
            Some(VersionRefKind::Commit(hex)) => {
                Hash::from_hex(&hex).map_err(|_| {
                    Status::invalid_argument(format!("invalid commit hash: {hex}"))
                })
            }
            Some(VersionRefKind::Tag(tag)) => {
                let key = schemahub_storage::keys::tag_ref_key(project, repo, &tag);
                core.storage
                    .get_ref(&key)
                    .map_err(|e| Status::internal(e.to_string()))?
                    .ok_or_else(|| Status::not_found(format!("tag '{tag}' not found")))
            }
            None => {
                // Default to main branch.
                core.get_branch_head(project, repo, "main")
                    .map_err(core_to_status)
            }
        },
        None => {
            core.get_branch_head(project, repo, "main")
                .map_err(core_to_status)
        }
    }
}

/// Convert a CommitObject to proto CommitInfo.
fn commit_object_to_proto(
    hash: &Hash,
    commit: &schemahub_core::objects::CommitObject,
) -> CommitInfo {
    CommitInfo {
        hash: hash.to_hex(),
        parent_hashes: commit.parent_hashes.clone(),
        timestamp: Some(Timestamp {
            seconds: commit.timestamp_unix,
            nanos: 0,
        }),
        author: commit.author.clone(),
        message: commit.message.clone(),
        force: commit.force,
        format_id: commit.format_id.clone(),
    }
}

#[tonic::async_trait]
impl RefService for RefServiceImpl {
    async fn get_commit(
        &self,
        request: Request<GetCommitRequest>,
    ) -> Result<Response<GetCommitResponse>, Status> {
        let req = request.into_inner();
        let commit_obj = self.core
            .get_commit(&req.project, &req.repo, &req.commit)
            .map_err(core_to_status)?;
        let hash = Hash::from_hex(&req.commit)
            .map_err(|_| Status::invalid_argument(format!("invalid commit hash: {}", req.commit)))?;
        let commit_info = commit_object_to_proto(&hash, &commit_obj);
        Ok(Response::new(GetCommitResponse {
            commit: Some(commit_info),
        }))
    }

    type ListCommitsStream = BoxStream<CommitInfo>;

    async fn list_commits(
        &self,
        request: Request<ListCommitsRequest>,
    ) -> Result<Response<Self::ListCommitsStream>, Status> {
        let req = request.into_inner();

        // `from` is a VersionRef (branch/tag/commit); resolve to commit hex for list_commits.
        let from_hex: Option<String> = if req.from.is_some() {
            Some(resolve_version_ref(&self.core, &req.project, &req.repo, req.from)?.to_hex())
        } else {
            None
        };
        let from_branch: Option<&str> = None; // we use from_commit (resolved) instead
        let from_commit: Option<&str> = from_hex.as_deref();
        let limit = 50_usize;

        let commits = self.core
            .list_commits(&req.project, &req.repo, from_branch, from_commit, limit)
            .map_err(core_to_status)?;

        let items: Vec<Result<CommitInfo, Status>> = commits
            .into_iter()
            .map(|(hash, commit_obj)| Ok(commit_object_to_proto(&hash, &commit_obj)))
            .collect();

        let stream = tokio_stream::iter(items);
        Ok(Response::new(Box::pin(stream)))
    }

    async fn diff(
        &self,
        _request: Request<DiffRequest>,
    ) -> Result<Response<DiffResponse>, Status> {
        Err(Status::unimplemented("Diff not yet implemented"))
    }

    async fn create_branch(
        &self,
        request: Request<CreateBranchRequest>,
    ) -> Result<Response<CreateBranchResponse>, Status> {
        let req = request.into_inner();

        // Resolve the `from` VersionRef to a commit hash string.
        let from_hash = match req.from {
            Some(version_ref) => match version_ref.r#ref {
                Some(VersionRefKind::Branch(branch_name)) => {
                    let h = self
                        .core
                        .get_branch_head(&req.project, &req.repo, &branch_name)
                        .map_err(core_to_status)?;
                    h.to_hex()
                }
                Some(VersionRefKind::Commit(hash)) => hash,
                Some(VersionRefKind::Tag(_)) => {
                    return Err(Status::unimplemented(
                        "CreateBranch from tag not yet implemented",
                    ));
                }
                None => {
                    return Err(Status::invalid_argument(
                        "CreateBranchRequest.from.ref must be set",
                    ));
                }
            },
            None => {
                return Err(Status::invalid_argument(
                    "CreateBranchRequest.from must be set",
                ));
            }
        };

        self.core
            .create_branch(&req.project, &req.repo, &req.name, &from_hash)
            .map_err(core_to_status)?;

        // Re-read the branch head to return BranchInfo.
        let head_commit = self
            .core
            .get_branch_head(&req.project, &req.repo, &req.name)
            .map_err(core_to_status)?;

        let branch = BranchInfo {
            project: req.project,
            repo: req.repo,
            name: req.name,
            head_commit: head_commit.to_hex(),
            protected: false,
        };
        Ok(Response::new(CreateBranchResponse {
            branch: Some(branch),
        }))
    }

    async fn delete_branch(
        &self,
        request: Request<DeleteBranchRequest>,
    ) -> Result<Response<DeleteBranchResponse>, Status> {
        let req = request.into_inner();
        self.core
            .delete_branch(&req.project, &req.repo, &req.name)
            .map_err(core_to_status)?;
        Ok(Response::new(DeleteBranchResponse {}))
    }

    async fn list_branches(
        &self,
        request: Request<ListBranchesRequest>,
    ) -> Result<Response<ListBranchesResponse>, Status> {
        let req = request.into_inner();
        let branches = self
            .core
            .list_branches(&req.project, &req.repo, &req.name_prefix)
            .map_err(core_to_status)?;

        let branch_infos = branches
            .into_iter()
            .map(|(name, hash)| BranchInfo {
                project: req.project.clone(),
                repo: req.repo.clone(),
                name,
                head_commit: hash.to_hex(),
                protected: false,
            })
            .collect();

        Ok(Response::new(ListBranchesResponse {
            branches: branch_infos,
        }))
    }

    async fn get_branch(
        &self,
        request: Request<GetBranchRequest>,
    ) -> Result<Response<GetBranchResponse>, Status> {
        let req = request.into_inner();
        let head = self
            .core
            .get_branch_head(&req.project, &req.repo, &req.name)
            .map_err(core_to_status)?;

        let branch = BranchInfo {
            project: req.project,
            repo: req.repo,
            name: req.name,
            head_commit: head.to_hex(),
            protected: false,
        };
        Ok(Response::new(GetBranchResponse {
            branch: Some(branch),
        }))
    }

    async fn create_tag(
        &self,
        request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        let req = request.into_inner();

        // Resolve the commit from the VersionRef.
        let commit_hash = resolve_version_ref(&self.core, &req.project, &req.repo, req.target)?;
        let commit_hex = commit_hash.to_hex();

        self.core
            .create_tag(&req.project, &req.repo, &req.name, &commit_hex, None)
            .map_err(core_to_status)?;

        let tag = TagInfo {
            project: req.project,
            repo: req.repo,
            name: req.name,
            commit_hash: commit_hex,
            annotated: false,
            tagger: String::new(),
            message: String::new(),
            timestamp: None,
        };
        Ok(Response::new(CreateTagResponse { tag: Some(tag) }))
    }

    async fn delete_tag(
        &self,
        request: Request<DeleteTagRequest>,
    ) -> Result<Response<DeleteTagResponse>, Status> {
        let req = request.into_inner();
        self.core
            .delete_tag(&req.project, &req.repo, &req.name)
            .map_err(core_to_status)?;
        Ok(Response::new(DeleteTagResponse {}))
    }

    async fn list_tags(
        &self,
        request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        let req = request.into_inner();
        let tags = self.core
            .list_tags(&req.project, &req.repo, &req.name_prefix)
            .map_err(core_to_status)?;

        let tag_infos: Vec<TagInfo> = tags
            .into_iter()
            .map(|(name, hash)| TagInfo {
                project: req.project.clone(),
                repo: req.repo.clone(),
                name,
                commit_hash: hash.to_hex(),
                annotated: false,
                tagger: String::new(),
                message: String::new(),
                timestamp: None,
            })
            .collect();

        Ok(Response::new(ListTagsResponse { tags: tag_infos }))
    }

    async fn merge(
        &self,
        request: Request<MergeRequest>,
    ) -> Result<Response<MergeResponse>, Status> {
        let req = request.into_inner();

        let new_head = self.core
            .merge_branches(
                &req.project,
                &req.repo,
                &req.source_branch,
                &req.target_branch,
                &req.base_revision,
                &req.idempotency_key,
                "schemahub-server",
                None,
            )
            .map_err(core_to_status)?;

        Ok(Response::new(MergeResponse { new_commit: new_head }))
    }
}
