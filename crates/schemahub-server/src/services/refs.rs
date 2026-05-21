use std::collections::HashMap;
use std::sync::Arc;

use prost_types::Timestamp;
use schemahub_core::Core;
use schemahub_core::objects::{KIND_BLOB, KIND_SUBTREE};
use schemahub_types::Hash;
use tonic::{Request, Response, Status};

use schemahub_api::schemahub_v1::{
    BranchInfo, CommitInfo, CreateBranchRequest, CreateBranchResponse, CreateTagRequest,
    CreateTagResponse, DeclarationChange, DeleteBranchRequest, DeleteBranchResponse,
    DeleteTagRequest, DeleteTagResponse, DiffRequest, DiffResponse, GetBranchRequest,
    GetBranchResponse, GetCommitRequest, GetCommitResponse, ListBranchesRequest,
    ListBranchesResponse, ListCommitsRequest, ListTagsRequest, ListTagsResponse, MergeRequest,
    MergeResponse, SchemaDiff, TagInfo,
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
        request: Request<DiffRequest>,
    ) -> Result<Response<DiffResponse>, Status> {
        let req = request.into_inner();

        // Resolve base and head commit hashes.
        let base_hash = resolve_version_ref(&self.core, &req.project, &req.repo, req.base)?;
        let head_hash = resolve_version_ref(&self.core, &req.project, &req.repo, req.head)?;

        if base_hash == head_hash {
            return Ok(Response::new(DiffResponse { schema_diffs: vec![] }));
        }

        // Read root trees for both commits.
        let base_commit = self.core
            .get_commit(&req.project, &req.repo, &base_hash.to_hex())
            .map_err(core_to_status)?;
        let head_commit = self.core
            .get_commit(&req.project, &req.repo, &head_hash.to_hex())
            .map_err(core_to_status)?;

        let base_tree = {
            let storage = self.core.storage.as_ref();
            let commit_obj = schemahub_core::objects::decode_commit(
                &storage.read_object(&base_hash)
                    .map_err(|e| Status::internal(e.to_string()))?
                    .ok_or_else(|| Status::not_found("base commit not found"))?,
            ).map_err(|e| Status::internal(e.to_string()))?;
            let tree_hash = Hash::from_hex(&commit_obj.tree_hash)
                .map_err(|_| Status::internal("invalid base tree hash"))?;
            let tree_data = storage.read_object(&tree_hash)
                .map_err(|e| Status::internal(e.to_string()))?
                .ok_or_else(|| Status::not_found("base root tree not found"))?;
            schemahub_core::objects::decode_tree(&tree_data)
                .map_err(|e| Status::internal(e.to_string()))?
        };
        let head_tree = {
            let storage = self.core.storage.as_ref();
            let commit_obj = schemahub_core::objects::decode_commit(
                &storage.read_object(&head_hash)
                    .map_err(|e| Status::internal(e.to_string()))?
                    .ok_or_else(|| Status::not_found("head commit not found"))?,
            ).map_err(|e| Status::internal(e.to_string()))?;
            let tree_hash = Hash::from_hex(&commit_obj.tree_hash)
                .map_err(|_| Status::internal("invalid head tree hash"))?;
            let tree_data = storage.read_object(&tree_hash)
                .map_err(|e| Status::internal(e.to_string()))?
                .ok_or_else(|| Status::not_found("head root tree not found"))?;
            schemahub_core::objects::decode_tree(&tree_data)
                .map_err(|e| Status::internal(e.to_string()))?
        };

        // Build maps: schema_name → hash for base and head.
        let base_schemas: HashMap<String, String> = base_tree.entries.iter()
            .filter(|e| e.kind == KIND_SUBTREE)
            .map(|e| (e.name.clone(), e.hash.clone()))
            .collect();
        let head_schemas: HashMap<String, String> = head_tree.entries.iter()
            .filter(|e| e.kind == KIND_SUBTREE)
            .map(|e| (e.name.clone(), e.hash.clone()))
            .collect();

        let mut schema_diffs = Vec::new();

        // Determine which schemas to diff.
        let mut all_schema_names: Vec<String> = base_schemas.keys()
            .chain(head_schemas.keys())
            .cloned()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        all_schema_names.sort();

        if !req.schema_path.is_empty() {
            all_schema_names.retain(|n| n == &req.schema_path);
        }

        let storage = self.core.storage.as_ref();

        for schema_name in all_schema_names {
            let base_hash_opt = base_schemas.get(&schema_name);
            let head_hash_opt = head_schemas.get(&schema_name);

            // Skip schemas that are identical.
            if base_hash_opt == head_hash_opt {
                continue;
            }

            let mut changes = Vec::new();

            // Load declaration-level entries from both trees.
            let base_decls: HashMap<String, String> = match base_hash_opt {
                None => HashMap::new(),
                Some(h) => {
                    let tree_hash = Hash::from_hex(h)
                        .map_err(|_| Status::internal(format!("invalid schema tree hash for {schema_name}")))?;
                    let tree_data = storage.read_object(&tree_hash)
                        .map_err(|e| Status::internal(e.to_string()))?
                        .unwrap_or_default();
                    schemahub_core::objects::decode_tree(&tree_data)
                        .map(|t| t.entries.into_iter()
                            .filter(|e| e.kind == KIND_BLOB)
                            .map(|e| (e.name, e.hash))
                            .collect())
                        .unwrap_or_default()
                }
            };
            let head_decls: HashMap<String, String> = match head_hash_opt {
                None => HashMap::new(),
                Some(h) => {
                    let tree_hash = Hash::from_hex(h)
                        .map_err(|_| Status::internal(format!("invalid schema tree hash for {schema_name}")))?;
                    let tree_data = storage.read_object(&tree_hash)
                        .map_err(|e| Status::internal(e.to_string()))?
                        .unwrap_or_default();
                    schemahub_core::objects::decode_tree(&tree_data)
                        .map(|t| t.entries.into_iter()
                            .filter(|e| e.kind == KIND_BLOB)
                            .map(|e| (e.name, e.hash))
                            .collect())
                        .unwrap_or_default()
                }
            };

            // Find added, removed, and modified declarations.
            let mut all_decl_names: Vec<String> = base_decls.keys()
                .chain(head_decls.keys())
                .cloned()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            all_decl_names.sort();

            for decl_name in all_decl_names {
                let in_base = base_decls.get(&decl_name);
                let in_head = head_decls.get(&decl_name);
                let change_type = match (in_base, in_head) {
                    (None, Some(_)) => "added",
                    (Some(_), None) => "removed",
                    (Some(bh), Some(hh)) if bh != hh => "modified",
                    _ => continue, // identical hash, no change
                };
                changes.push(DeclarationChange {
                    change_type: change_type.to_string(),
                    decl_name,
                    detail: vec![],
                });
            }

            if !changes.is_empty() {
                schema_diffs.push(SchemaDiff { schema_path: schema_name, changes });
            }
        }

        // Suppress unused variable warnings for commit objects we loaded but only used
        // for tree retrieval via the storage path.
        let _ = (base_commit, head_commit);

        Ok(Response::new(DiffResponse { schema_diffs }))
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
