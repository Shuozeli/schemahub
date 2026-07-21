//! Immutable revision resolution and artifact serving.

use std::sync::Arc;

use schemahub_api::schemahub_v1 as pb;
use schemahub_api::schemahub_v1::serving_service_server::ServingService;
use schemahub_core::{Core, SchemaArtifactKind};
use schemahub_jj::RefSpec;
use schemahub_types::{Action, CodegenOptions};
use tonic::metadata::MetadataValue;
use tonic::{Request, Response, Status};

use crate::error::to_status;
use crate::services::{refspec_or_repository_default, token_from};
use crate::wire;

pub struct ServingHandler {
    core: Arc<Core>,
}

impl ServingHandler {
    pub fn new(core: Arc<Core>) -> Self {
        Self { core }
    }
}

#[tonic::async_trait]
impl ServingService for ServingHandler {
    async fn resolve_revision(
        &self,
        request: Request<pb::ResolveRevisionRequest>,
    ) -> Result<Response<pb::SchemaRevision>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let (project, repo) = parse_parent(&request.parent)?;
        let at = refspec_or_repository_default(
            &self.core,
            project,
            repo,
            &request.at,
            Action::Read,
            token.as_deref(),
        )?;
        let revision = self
            .core
            .resolve_schema_revision(project, repo, &at, ref_label(&at), token.as_deref())
            .map_err(to_status)?;
        Ok(Response::new(pb::SchemaRevision {
            name: revision.name,
            project: revision.project,
            repo: revision.repo,
            commit_id: revision.commit_id,
            resolved_from: revision.resolved_from,
        }))
    }

    async fn get_schema_artifact(
        &self,
        request: Request<pb::GetSchemaArtifactRequest>,
    ) -> Result<Response<pb::SchemaArtifact>, Status> {
        let token = token_from(&request)?;
        let request = request.into_inner();
        let kind = artifact_kind_from_proto(request.kind)?;
        let language = if kind == SchemaArtifactKind::GeneratedCode {
            let language = pb::Language::try_from(request.language).map_err(|_| {
                Status::invalid_argument(format!("unknown language value {}", request.language))
            })?;
            Some(wire::language_from_pb(language)?)
        } else {
            None
        };
        let artifact = self
            .core
            .get_schema_artifact(
                &request.revision,
                &request.schema_path,
                kind,
                language,
                &CodegenOptions {
                    rust_pluggable_buffer: request.rust_pluggable_buffer,
                },
                token.as_deref(),
            )
            .map_err(to_status)?;
        let not_modified =
            !request.if_none_match.is_empty() && request.if_none_match == artifact.artifact_digest;
        let digest = artifact.artifact_digest.clone();
        let mut response = Response::new(pb::SchemaArtifact {
            name: artifact.name,
            revision: artifact.revision,
            schema_path: artifact.schema_path,
            kind: artifact_kind_to_proto(artifact.kind) as i32,
            format: format_to_proto(&artifact.format_id) as i32,
            media_type: artifact.media_type,
            content: if not_modified {
                Vec::new()
            } else {
                artifact.content.to_vec()
            },
            artifact_digest: artifact.artifact_digest,
            closure_digest: artifact.closure_digest,
            dependency_schemas: artifact.dependency_schemas,
            is_archive: artifact.is_archive,
            not_modified,
        });
        response.metadata_mut().insert(
            "x-schemahub-artifact-digest",
            MetadataValue::try_from(digest.as_str())
                .map_err(|_| Status::internal("artifact digest is invalid metadata"))?,
        );
        Ok(response)
    }
}

fn parse_parent(parent: &str) -> Result<(&str, &str), Status> {
    let parts: Vec<_> = parent.split('/').collect();
    if parts.len() != 4
        || parts[0] != "projects"
        || parts[2] != "repos"
        || parts[1].is_empty()
        || parts[3].is_empty()
    {
        return Err(Status::invalid_argument(
            "parent must be projects/{project}/repos/{repo}",
        ));
    }
    Ok((parts[1], parts[3]))
}

fn ref_label(at: &RefSpec) -> String {
    match at {
        RefSpec::Bookmark(name) => format!("branch:{name}"),
        RefSpec::Tag(name) => format!("tag:{name}"),
        RefSpec::Commit(id) => format!("commit:{id}"),
    }
}

fn artifact_kind_from_proto(value: i32) -> Result<SchemaArtifactKind, Status> {
    match pb::SchemaArtifactKind::try_from(value) {
        Ok(pb::SchemaArtifactKind::Source) => Ok(SchemaArtifactKind::Source),
        Ok(pb::SchemaArtifactKind::Descriptors) => Ok(SchemaArtifactKind::Descriptors),
        Ok(pb::SchemaArtifactKind::GeneratedCode) => Ok(SchemaArtifactKind::GeneratedCode),
        Ok(pb::SchemaArtifactKind::Unspecified) => Err(Status::invalid_argument(
            "artifact kind must be source, descriptors, or generated_code",
        )),
        Err(_) => Err(Status::invalid_argument(format!(
            "unknown artifact kind value {value}"
        ))),
    }
}

fn artifact_kind_to_proto(kind: SchemaArtifactKind) -> pb::SchemaArtifactKind {
    match kind {
        SchemaArtifactKind::Source => pb::SchemaArtifactKind::Source,
        SchemaArtifactKind::Descriptors => pb::SchemaArtifactKind::Descriptors,
        SchemaArtifactKind::GeneratedCode => pb::SchemaArtifactKind::GeneratedCode,
    }
}

fn format_to_proto(format_id: &str) -> pb::SchemaFormat {
    match format_id {
        "protobuf" => pb::SchemaFormat::Protobuf,
        "flatbuffers" => pb::SchemaFormat::Flatbuffers,
        "openapi" => pb::SchemaFormat::Openapi,
        _ => pb::SchemaFormat::Unspecified,
    }
}
