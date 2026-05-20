use std::sync::Arc;

use schemahub_core::Core;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    BranchInfo, CommitInfo, CreateBranchRequest, CreateBranchResponse, CreateTagRequest,
    CreateTagResponse, DeleteBranchRequest, DeleteBranchResponse, DeleteTagRequest,
    DeleteTagResponse, DiffRequest, DiffResponse, GetBranchRequest, GetBranchResponse,
    GetCommitRequest, GetCommitResponse, ListBranchesRequest, ListBranchesResponse,
    ListCommitsRequest, ListTagsRequest, ListTagsResponse, MergeRequest, MergeResponse,
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

fn _empty_stream<T: Send + 'static>() -> BoxStream<T> {
    Box::pin(tokio_stream::empty::<Result<T, Status>>())
}

#[tonic::async_trait]
impl RefService for RefServiceImpl {
    async fn get_commit(
        &self,
        _request: Request<GetCommitRequest>,
    ) -> Result<Response<GetCommitResponse>, Status> {
        Err(Status::unimplemented("GetCommit not yet implemented"))
    }

    type ListCommitsStream = BoxStream<CommitInfo>;

    async fn list_commits(
        &self,
        _request: Request<ListCommitsRequest>,
    ) -> Result<Response<Self::ListCommitsStream>, Status> {
        Err(Status::unimplemented("ListCommits not yet implemented"))
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
        _request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        Err(Status::unimplemented("CreateTag not yet implemented"))
    }

    async fn delete_tag(
        &self,
        _request: Request<DeleteTagRequest>,
    ) -> Result<Response<DeleteTagResponse>, Status> {
        Err(Status::unimplemented("DeleteTag not yet implemented"))
    }

    async fn list_tags(
        &self,
        _request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        Err(Status::unimplemented("ListTags not yet implemented"))
    }

    async fn merge(
        &self,
        _request: Request<MergeRequest>,
    ) -> Result<Response<MergeResponse>, Status> {
        Err(Status::unimplemented("Merge not yet implemented"))
    }
}
