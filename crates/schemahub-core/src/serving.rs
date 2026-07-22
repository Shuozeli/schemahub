//! Immutable revision resolution and deterministic schema artifact serving.

use std::sync::Arc;

use bytes::Bytes;
use schemahub_jj::{ObjectDb, ObjectDbError, RefSpec};
use schemahub_types::{Action, CodegenOptions, Language, SchemaClosure, SchemaPath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::closure;
use crate::repository::RepositoryError;
use crate::Core;

const ARTIFACT_COLLECTION: &str = "schemahub.artifacts.v1";
const ARTIFACT_RECORD_MAGIC: &[u8] = b"schemahub-artifact-record-v1\0";
const ARTIFACT_RECORD_VERSION: u32 = 1;
const MAX_ARTIFACT_METADATA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaArtifactKind {
    Source,
    Descriptors,
    GeneratedCode,
}

impl SchemaArtifactKind {
    fn resource_component(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Descriptors => "descriptors",
            Self::GeneratedCode => "generated-code",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaRevision {
    pub name: String,
    pub project: String,
    pub repo: String,
    pub commit_id: String,
    pub resolved_from: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaArtifact {
    pub name: String,
    pub revision: String,
    pub schema_path: String,
    pub kind: SchemaArtifactKind,
    pub format_id: String,
    pub media_type: String,
    pub content: Bytes,
    pub artifact_digest: String,
    pub closure_digest: String,
    pub dependency_schemas: Vec<String>,
    pub is_archive: bool,
}

/// Canonical request identity stored with the first successful materialization.
/// Adding a code-generation option requires a new request-key version or an
/// explicit field here; silently aliasing distinct renderer inputs is forbidden.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ArtifactRequestRecord {
    version: u32,
    revision: String,
    schema_path: String,
    kind: String,
    language: Option<String>,
    rust_pluggable_buffer: bool,
}

#[derive(Clone, Debug)]
struct ArtifactRequest {
    record: ArtifactRequestRecord,
    storage_key: String,
    kind: SchemaArtifactKind,
    language: Option<Language>,
    options: CodegenOptions,
}

impl ArtifactRequest {
    fn new(
        revision: &str,
        schema_path: &str,
        kind: SchemaArtifactKind,
        language: Option<Language>,
        options: &CodegenOptions,
    ) -> CoreResult<Self> {
        let (language, options) = match kind {
            SchemaArtifactKind::GeneratedCode => {
                let language = language.ok_or_else(|| {
                    CoreError::InvalidArgument(
                        "generated-code artifacts require a language".to_string(),
                    )
                })?;
                (Some(language), options.clone())
            }
            SchemaArtifactKind::Source | SchemaArtifactKind::Descriptors => {
                (None, CodegenOptions::default())
            }
        };
        let record = ArtifactRequestRecord {
            version: ARTIFACT_RECORD_VERSION,
            revision: revision.to_string(),
            schema_path: schema_path.to_string(),
            kind: kind.resource_component().to_string(),
            language: language.map(language_name).map(str::to_string),
            rust_pluggable_buffer: options.rust_pluggable_buffer,
        };
        let mut encoded = Vec::new();
        push_part(&mut encoded, b"schemahub-artifact-request-v1");
        push_part(&mut encoded, record.revision.as_bytes());
        push_part(&mut encoded, record.schema_path.as_bytes());
        push_part(&mut encoded, record.kind.as_bytes());
        push_part(
            &mut encoded,
            record.language.as_deref().unwrap_or("none").as_bytes(),
        );
        push_part(&mut encoded, &[u8::from(record.rust_pluggable_buffer)]);
        let storage_key = sha256_digest(&encoded)
            .strip_prefix("sha256:")
            .expect("sha256_digest always has its algorithm prefix")
            .to_string();
        Ok(Self {
            record,
            storage_key,
            kind,
            language,
            options,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredArtifact {
    artifact: SchemaArtifact,
    dependency_paths: Vec<SchemaPath>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ArtifactRecordMetadata {
    version: u32,
    request: ArtifactRequestRecord,
    format_id: String,
    media_type: String,
    artifact_digest: String,
    closure_digest: String,
    dependency_paths: Vec<SchemaPath>,
    is_archive: bool,
}

/// First-materialization store over the same ObjectDb as JJ and the mutable
/// control plane. `create_record` is atomic on every backend, so concurrent or
/// mixed-version servers converge on the first successfully persisted bytes.
pub(crate) struct ArtifactMaterializationStore {
    db: Arc<dyn ObjectDb>,
}

impl ArtifactMaterializationStore {
    pub(crate) fn new(db: Arc<dyn ObjectDb>) -> Self {
        Self { db }
    }

    fn load(&self, request: &ArtifactRequest) -> CoreResult<Option<StoredArtifact>> {
        self.db
            .get_record(ARTIFACT_COLLECTION, &request.storage_key)
            .map_err(artifact_store_error)?
            .map(|bytes| decode_artifact_record(request, &bytes))
            .transpose()
    }

    fn insert_first(
        &self,
        request: &ArtifactRequest,
        candidate: StoredArtifact,
    ) -> CoreResult<StoredArtifact> {
        let encoded = encode_artifact_record(request, &candidate)?;
        if self
            .db
            .create_record(ARTIFACT_COLLECTION, &request.storage_key, &encoded)
            .map_err(artifact_store_error)?
        {
            tracing::info!(
                event = "schemahub.artifact.materialized",
                request_key = request.storage_key,
                artifact_name = candidate.artifact.name,
                artifact_digest = candidate.artifact.artifact_digest,
                "first immutable artifact materialization persisted"
            );
            return Ok(candidate);
        }
        let winner = self.load(request)?.ok_or_else(|| {
            CoreError::Other(format!(
                "artifact materialization {:?} won by another writer but is unreadable",
                request.storage_key
            ))
        })?;
        tracing::info!(
            event = "schemahub.artifact.materialization_race_resolved",
            request_key = request.storage_key,
            artifact_name = winner.artifact.name,
            artifact_digest = winner.artifact.artifact_digest,
            "concurrent artifact materialization returned the persisted winner"
        );
        Ok(winner)
    }
}

impl Core {
    /// Resolve one authorized repository read to an immutable, repository-owned
    /// commit. All public read flows use this before touching schema objects:
    /// content objects are globally deduplicated, so merely loading a raw
    /// commit id does not prove that it belongs to the requested repository.
    pub(crate) fn resolve_read_commit(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<String> {
        validate_segment("project", project)?;
        validate_segment("repo", repo)?;
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        self.effective_repo_config(project, repo)?;
        // The JJ boundary additionally proves repository ownership for raw
        // commit ids; bookmark and tag targets are scoped by the repo view.
        Ok(self.jj.resolve_ref_id(project, repo, at)?)
    }

    /// Resolve a mutable or immutable ref once and return a repository-scoped
    /// immutable revision resource.
    pub fn resolve_schema_revision(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        resolved_from: String,
        token: Option<&str>,
    ) -> CoreResult<SchemaRevision> {
        let commit_id = self.resolve_read_commit(project, repo, at, token)?;
        Ok(SchemaRevision {
            name: format!("projects/{project}/repos/{repo}/revisions/{commit_id}"),
            project: project.to_string(),
            repo: repo.to_string(),
            commit_id,
            resolved_from,
        })
    }

    /// Return an artifact solely from an immutable revision. The first
    /// successful materialization is durably inserted before it is returned;
    /// every later request (including after a compiler upgrade) reads those
    /// exact bytes. The response carries both payload and closure digests.
    #[allow(clippy::too_many_arguments)]
    pub fn get_schema_artifact(
        &self,
        revision_name: &str,
        schema_name: &str,
        kind: SchemaArtifactKind,
        language: Option<Language>,
        options: &CodegenOptions,
        token: Option<&str>,
    ) -> CoreResult<SchemaArtifact> {
        let (project, repo, commit_id) = parse_revision_name(revision_name)?;
        validate_schema_name("schema_path", schema_name)?;
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        let serving_policy = self.effective_repo_config(project, repo)?.serving_policy;
        let allowed = match kind {
            SchemaArtifactKind::Source => serving_policy.source,
            SchemaArtifactKind::Descriptors => serving_policy.descriptors,
            SchemaArtifactKind::GeneratedCode => serving_policy.generated_code,
        };
        if !allowed {
            return Err(RepositoryError::FailedPrecondition(format!(
                "repository serving policy disables {} artifacts",
                kind.resource_component()
            ))
            .into());
        }
        self.jj.validate_revision(project, repo, commit_id)?;

        let request = ArtifactRequest::new(revision_name, schema_name, kind, language, options)?;
        if let Some(stored) = self.artifact_store.load(&request)? {
            self.authorize_artifact_dependencies(&stored.dependency_paths, token)?;
            tracing::debug!(
                event = "schemahub.artifact.materialization_hit",
                request_key = request.storage_key,
                artifact_name = stored.artifact.name,
                artifact_digest = stored.artifact.artifact_digest,
                "immutable artifact served from first-materialization storage"
            );
            return Ok(stored.artifact);
        }

        let schema = SchemaPath::new(project, repo, schema_name);
        let compiler = self.compiler_for(schema_name)?;
        let closure = closure::build(self, &schema, commit_id, token)?;
        let closure_digest = closure_digest(&closure);
        let content = match kind {
            SchemaArtifactKind::Source => {
                let root = closure.entries.get(&schema).ok_or_else(|| {
                    CoreError::Other("resolved closure is missing its root schema".to_string())
                })?;
                Bytes::from(compiler.print(root)?.into_bytes())
            }
            SchemaArtifactKind::Descriptors => compiler.generate_descriptors(&closure)?,
            SchemaArtifactKind::GeneratedCode => {
                let language = request
                    .language
                    .expect("ArtifactRequest validates generated-code language");
                Bytes::from(
                    compiler
                        .generate_code(&closure, language, &request.options)?
                        .into_bytes(),
                )
            }
        };
        let artifact_digest = sha256_digest(&content);
        let mut dependency_paths: Vec<_> = closure
            .entries
            .keys()
            .filter(|path| **path != schema)
            .cloned()
            .collect();
        dependency_paths.sort();
        let dependency_schemas = dependency_paths.iter().map(ToString::to_string).collect();
        let format_id = compiler.format_id().to_string();
        let media_type = artifact_media_type(&format_id, kind).to_string();
        let candidate = StoredArtifact {
            artifact: SchemaArtifact {
                name: artifact_resource_name(&request, &artifact_digest),
                revision: revision_name.to_string(),
                schema_path: schema_name.to_string(),
                kind,
                format_id,
                media_type,
                content,
                artifact_digest,
                closure_digest,
                dependency_schemas,
                is_archive: false,
            },
            dependency_paths,
        };
        let winner = self.artifact_store.insert_first(&request, candidate)?;
        // A different process may have won with a closure produced by another
        // installed renderer. Re-authorize the persisted winner's dependencies
        // before returning it rather than assuming the local candidate's set.
        self.authorize_artifact_dependencies(&winner.dependency_paths, token)?;
        Ok(winner.artifact)
    }

    fn authorize_artifact_dependencies(
        &self,
        dependencies: &[SchemaPath],
        token: Option<&str>,
    ) -> CoreResult<()> {
        for path in dependencies {
            authorize(
                self.authn.as_ref(),
                self.authz.as_ref(),
                token,
                Action::Read,
                &path.project,
                &path.repo,
            )?;
        }
        Ok(())
    }
}

fn artifact_resource_name(request: &ArtifactRequest, artifact_digest: &str) -> String {
    let resource_key = sha256_digest(
        format!(
            "schemahub-artifact-resource-v1\0{}\0{}\0{}\0{}\0{artifact_digest}",
            request.record.schema_path,
            request.record.kind,
            request.record.language.as_deref().unwrap_or("none"),
            request.record.rust_pluggable_buffer
        )
        .as_bytes(),
    );
    let resource_id = resource_key
        .strip_prefix("sha256:")
        .expect("sha256_digest always has its algorithm prefix");
    format!(
        "{}/artifacts/{}-{resource_id}",
        request.record.revision,
        request.kind.resource_component()
    )
}

fn encode_artifact_record(
    request: &ArtifactRequest,
    stored: &StoredArtifact,
) -> CoreResult<Vec<u8>> {
    validate_stored_artifact(request, stored)?;
    let metadata = ArtifactRecordMetadata {
        version: ARTIFACT_RECORD_VERSION,
        request: request.record.clone(),
        format_id: stored.artifact.format_id.clone(),
        media_type: stored.artifact.media_type.clone(),
        artifact_digest: stored.artifact.artifact_digest.clone(),
        closure_digest: stored.artifact.closure_digest.clone(),
        dependency_paths: stored.dependency_paths.clone(),
        is_archive: stored.artifact.is_archive,
    };
    let metadata = serde_json::to_vec(&metadata)
        .map_err(|error| CoreError::Other(format!("encoding artifact metadata: {error}")))?;
    if metadata.len() > MAX_ARTIFACT_METADATA_BYTES {
        return Err(CoreError::Other(format!(
            "artifact metadata exceeds {MAX_ARTIFACT_METADATA_BYTES} bytes"
        )));
    }
    let metadata_len = u64::try_from(metadata.len())
        .map_err(|_| CoreError::Other("artifact metadata length overflow".to_string()))?;
    let capacity = ARTIFACT_RECORD_MAGIC
        .len()
        .checked_add(8)
        .and_then(|value| value.checked_add(metadata.len()))
        .and_then(|value| value.checked_add(stored.artifact.content.len()))
        .ok_or_else(|| CoreError::Other("artifact record length overflow".to_string()))?;
    let mut encoded = Vec::with_capacity(capacity);
    encoded.extend_from_slice(ARTIFACT_RECORD_MAGIC);
    encoded.extend_from_slice(&metadata_len.to_be_bytes());
    encoded.extend_from_slice(&metadata);
    encoded.extend_from_slice(&stored.artifact.content);
    Ok(encoded)
}

fn decode_artifact_record(request: &ArtifactRequest, encoded: &[u8]) -> CoreResult<StoredArtifact> {
    let length_start = ARTIFACT_RECORD_MAGIC.len();
    let length_end = length_start
        .checked_add(8)
        .ok_or_else(|| CoreError::Other("artifact record length prefix overflow".to_string()))?;
    if !encoded.starts_with(ARTIFACT_RECORD_MAGIC) || encoded.len() < length_end {
        return Err(corrupt_artifact("invalid record header"));
    }
    let length_bytes: [u8; 8] = encoded[length_start..length_end]
        .try_into()
        .map_err(|_| corrupt_artifact("invalid metadata length prefix"))?;
    let metadata_len = usize::try_from(u64::from_be_bytes(length_bytes))
        .map_err(|_| corrupt_artifact("metadata length does not fit this platform"))?;
    if metadata_len > MAX_ARTIFACT_METADATA_BYTES {
        return Err(corrupt_artifact("metadata length exceeds the safety bound"));
    }
    let content_start = length_end
        .checked_add(metadata_len)
        .filter(|end| *end <= encoded.len())
        .ok_or_else(|| corrupt_artifact("truncated metadata"))?;
    let metadata: ArtifactRecordMetadata =
        serde_json::from_slice(&encoded[length_end..content_start])
            .map_err(|error| corrupt_artifact(&format!("invalid metadata JSON: {error}")))?;
    if metadata.version != ARTIFACT_RECORD_VERSION {
        return Err(corrupt_artifact(&format!(
            "unsupported record version {}",
            metadata.version
        )));
    }
    if metadata.request != request.record {
        return Err(corrupt_artifact(
            "stored request identity does not match its collection key",
        ));
    }
    let content = Bytes::copy_from_slice(&encoded[content_start..]);
    let dependency_schemas = metadata
        .dependency_paths
        .iter()
        .map(ToString::to_string)
        .collect();
    let stored = StoredArtifact {
        artifact: SchemaArtifact {
            name: artifact_resource_name(request, &metadata.artifact_digest),
            revision: request.record.revision.clone(),
            schema_path: request.record.schema_path.clone(),
            kind: request.kind,
            format_id: metadata.format_id,
            media_type: metadata.media_type,
            content,
            artifact_digest: metadata.artifact_digest,
            closure_digest: metadata.closure_digest,
            dependency_schemas,
            is_archive: metadata.is_archive,
        },
        dependency_paths: metadata.dependency_paths,
    };
    validate_stored_artifact(request, &stored)?;
    Ok(stored)
}

fn validate_stored_artifact(request: &ArtifactRequest, stored: &StoredArtifact) -> CoreResult<()> {
    let artifact = &stored.artifact;
    if artifact.revision != request.record.revision
        || artifact.schema_path != request.record.schema_path
        || artifact.kind != request.kind
    {
        return Err(corrupt_artifact(
            "artifact scope does not match its request identity",
        ));
    }
    if artifact.format_id.is_empty()
        || artifact.format_id.len() > 64
        || artifact.format_id.chars().any(char::is_control)
    {
        return Err(corrupt_artifact("invalid format identifier"));
    }
    if artifact.media_type.is_empty()
        || artifact.media_type.len() > 256
        || artifact.media_type.chars().any(char::is_control)
    {
        return Err(corrupt_artifact("invalid media type"));
    }
    let computed_digest = sha256_digest(&artifact.content);
    if artifact.artifact_digest != computed_digest {
        return Err(corrupt_artifact("content digest mismatch"));
    }
    validate_sha256_digest("closure", &artifact.closure_digest)?;
    if artifact.name != artifact_resource_name(request, &artifact.artifact_digest) {
        return Err(corrupt_artifact("artifact resource name mismatch"));
    }
    let mut canonical_dependencies = stored.dependency_paths.clone();
    canonical_dependencies.sort();
    canonical_dependencies.dedup();
    if canonical_dependencies != stored.dependency_paths {
        return Err(corrupt_artifact("dependency paths are not uniquely sorted"));
    }
    for path in &stored.dependency_paths {
        validate_segment("dependency project", &path.project)
            .and_then(|()| validate_segment("dependency repo", &path.repo))
            .and_then(|()| validate_schema_name("dependency schema", &path.schema_name))
            .map_err(|error| corrupt_artifact(&format!("invalid dependency path: {error}")))?;
    }
    let dependency_schemas: Vec<_> = stored
        .dependency_paths
        .iter()
        .map(ToString::to_string)
        .collect();
    if artifact.dependency_schemas != dependency_schemas {
        return Err(corrupt_artifact("dependency metadata mismatch"));
    }
    Ok(())
}

fn validate_sha256_digest(label: &str, digest: &str) -> CoreResult<()> {
    let Some(value) = digest.strip_prefix("sha256:") else {
        return Err(corrupt_artifact(&format!(
            "{label} digest has no sha256 prefix"
        )));
    };
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(corrupt_artifact(&format!(
            "{label} digest is not lowercase SHA-256"
        )));
    }
    Ok(())
}

fn validate_schema_name(label: &str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(CoreError::InvalidArgument(format!(
            "{label} must not be empty or contain control characters"
        )));
    }
    Ok(())
}

fn corrupt_artifact(detail: &str) -> CoreError {
    CoreError::Other(format!("corrupt first-materialized artifact: {detail}"))
}

fn artifact_store_error(error: ObjectDbError) -> CoreError {
    CoreError::Other(format!("artifact materialization store: {error}"))
}

fn language_name(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::Go => "go",
        Language::TypeScript => "typescript",
        Language::Python => "python",
        Language::Java => "java",
    }
}

fn parse_revision_name(name: &str) -> CoreResult<(&str, &str, &str)> {
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() != 6 || parts[0] != "projects" || parts[2] != "repos" || parts[4] != "revisions"
    {
        return Err(CoreError::InvalidArgument(
            "revision must be projects/{project}/repos/{repo}/revisions/{commit}".to_string(),
        ));
    }
    validate_segment("project", parts[1])?;
    validate_segment("repo", parts[3])?;
    validate_segment("commit", parts[5])?;
    Ok((parts[1], parts[3], parts[5]))
}

fn validate_segment(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty() || value.contains('/') || value.chars().any(char::is_control) {
        return Err(CoreError::InvalidArgument(format!(
            "{label} must be a non-empty resource path segment without control characters"
        )));
    }
    Ok(())
}

fn artifact_media_type(format_id: &str, kind: SchemaArtifactKind) -> &'static str {
    match (format_id, kind) {
        ("protobuf", SchemaArtifactKind::Source) => "text/x-protobuf; charset=utf-8",
        ("flatbuffers", SchemaArtifactKind::Source) => "text/x-flatbuffers; charset=utf-8",
        ("openapi", SchemaArtifactKind::Source) => "application/yaml; charset=utf-8",
        ("protobuf", SchemaArtifactKind::Descriptors) => "application/x-protobuf",
        ("flatbuffers", SchemaArtifactKind::Descriptors) => {
            "application/vnd.schemahub.flatbuffers-bundle"
        }
        ("openapi", SchemaArtifactKind::Descriptors) => "application/yaml; charset=utf-8",
        (_, SchemaArtifactKind::GeneratedCode) => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Canonical v1 encoding: a version prefix followed by length-prefixed root,
/// path, meta, declaration-name, and declaration-blob byte strings. Entries
/// and declarations are lexically sorted before hashing.
fn closure_digest(closure: &SchemaClosure) -> String {
    let mut encoded = Vec::new();
    push_part(&mut encoded, b"schemahub-closure-v1");
    if let Some(root) = &closure.root {
        push_path(&mut encoded, root);
    } else {
        push_part(&mut encoded, b"");
    }
    let mut entries: Vec<_> = closure.entries.iter().collect();
    entries.sort_by_key(|(path, _)| *path);
    encoded.extend_from_slice(&(entries.len() as u64).to_be_bytes());
    for (path, schema) in entries {
        push_path(&mut encoded, path);
        push_part(&mut encoded, schema.meta.as_bytes());
        encoded.extend_from_slice(&(schema.decls.len() as u64).to_be_bytes());
        for (name, blob) in &schema.decls {
            push_part(&mut encoded, name.as_bytes());
            push_part(&mut encoded, blob.as_bytes());
        }
    }
    sha256_digest(&encoded)
}

fn push_path(encoded: &mut Vec<u8>, path: &SchemaPath) {
    push_part(encoded, path.project.as_bytes());
    push_part(encoded, path.repo.as_bytes());
    push_part(encoded, path.schema_name.as_bytes());
}

fn push_part(encoded: &mut Vec<u8>, part: &[u8]) {
    encoded.extend_from_slice(&(part.len() as u64).to_be_bytes());
    encoded.extend_from_slice(part);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use schemahub_jj::MemoryObjectDb;
    use schemahub_types::{DeclBlob, MetaBlob, SchemaObjects};

    use super::*;

    fn descriptor_request() -> ArtifactRequest {
        ArtifactRequest::new(
            "projects/p/repos/r/revisions/commit",
            "root.proto",
            SchemaArtifactKind::Descriptors,
            None,
            &CodegenOptions::default(),
        )
        .expect("valid descriptor request")
    }

    fn stored_descriptor(request: &ArtifactRequest, content: &'static [u8]) -> StoredArtifact {
        let content = Bytes::from_static(content);
        let artifact_digest = sha256_digest(&content);
        StoredArtifact {
            artifact: SchemaArtifact {
                name: artifact_resource_name(request, &artifact_digest),
                revision: request.record.revision.clone(),
                schema_path: request.record.schema_path.clone(),
                kind: request.kind,
                format_id: "protobuf".to_string(),
                media_type: "application/x-protobuf".to_string(),
                content,
                artifact_digest,
                closure_digest: sha256_digest(b"closure"),
                dependency_schemas: Vec::new(),
                is_archive: false,
            },
            dependency_paths: Vec::new(),
        }
    }

    #[test]
    fn closure_digest_is_independent_of_hashmap_insertion_order() {
        // Arrange
        let root = SchemaPath::new("p", "r", "root.proto");
        let dependency = SchemaPath::new("p", "r", "dep.proto");
        let root_objects = SchemaObjects {
            meta: MetaBlob::new(b"root-meta".to_vec()),
            decls: [("Root".to_string(), DeclBlob::new(b"root".to_vec()))]
                .into_iter()
                .collect(),
        };
        let dep_objects = SchemaObjects {
            meta: MetaBlob::new(b"dep-meta".to_vec()),
            decls: [("Dep".to_string(), DeclBlob::new(b"dep".to_vec()))]
                .into_iter()
                .collect(),
        };
        let left = SchemaClosure {
            root: Some(root.clone()),
            entries: HashMap::from([
                (root.clone(), root_objects.clone()),
                (dependency.clone(), dep_objects.clone()),
            ]),
        };
        let right = SchemaClosure {
            root: Some(root),
            entries: HashMap::from([
                (dependency, dep_objects),
                (SchemaPath::new("p", "r", "root.proto"), root_objects),
            ]),
        };

        // Act
        let left_digest = closure_digest(&left);
        let right_digest = closure_digest(&right);

        // Assert
        assert_eq!(left_digest, right_digest);
        assert!(left_digest.starts_with("sha256:"));
    }

    #[test]
    fn first_materialization_wins_across_renderer_instances() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let request = descriptor_request();
        let first_store = ArtifactMaterializationStore::new(db.clone());
        let first = stored_descriptor(&request, b"renderer-v1");
        first_store
            .insert_first(&request, first.clone())
            .expect("persist first renderer output");
        let upgraded_store = ArtifactMaterializationStore::new(db);
        let changed = stored_descriptor(&request, b"renderer-v2");

        // Act
        let winner = upgraded_store
            .insert_first(&request, changed)
            .expect("load first materialization");

        // Assert
        assert_eq!(winner, first);
        assert_eq!(winner.artifact.content, Bytes::from_static(b"renderer-v1"));
    }

    #[test]
    fn materialization_fails_closed_when_persisted_content_is_corrupt() {
        // Arrange
        let db: Arc<dyn ObjectDb> = Arc::new(MemoryObjectDb::new());
        let request = descriptor_request();
        let stored = stored_descriptor(&request, b"verified-content");
        let mut encoded = encode_artifact_record(&request, &stored).expect("encode fixture");
        *encoded.last_mut().expect("content byte") ^= 0xff;
        assert!(db
            .create_record(ARTIFACT_COLLECTION, &request.storage_key, &encoded)
            .expect("seed corrupt record"));
        let store = ArtifactMaterializationStore::new(db);

        // Act
        let error = store
            .load(&request)
            .expect_err("corrupt stored content must fail");

        // Assert
        assert!(error.to_string().contains("content digest mismatch"));
    }

    #[test]
    fn materialization_fails_closed_when_persisted_dependency_path_is_invalid() {
        // Arrange
        let request = descriptor_request();
        let mut stored = stored_descriptor(&request, b"verified-content");
        let invalid = SchemaPath::new("", "r", "dep.proto");
        stored.artifact.dependency_schemas = vec![invalid.to_string()];
        stored.dependency_paths = vec![invalid];

        // Act
        let error = validate_stored_artifact(&request, &stored)
            .expect_err("invalid stored dependency path must fail");

        // Assert
        assert!(error
            .to_string()
            .contains("corrupt first-materialized artifact: invalid dependency path"));
    }

    #[test]
    fn irrelevant_source_codegen_options_share_one_materialization_key() {
        // Arrange
        let default = CodegenOptions::default();
        let pluggable = CodegenOptions {
            rust_pluggable_buffer: true,
        };
        let plain = ArtifactRequest::new(
            "projects/p/repos/r/revisions/commit",
            "root.proto",
            SchemaArtifactKind::Source,
            None,
            &default,
        )
        .expect("plain source request");

        // Act
        let noisy = ArtifactRequest::new(
            "projects/p/repos/r/revisions/commit",
            "root.proto",
            SchemaArtifactKind::Source,
            Some(Language::Rust),
            &pluggable,
        )
        .expect("source request with ignored codegen fields");

        // Assert
        assert_eq!(plain.storage_key, noisy.storage_key);
        assert_eq!(plain.record, noisy.record);
    }

    #[test]
    fn generated_code_options_have_distinct_materialization_keys() {
        // Arrange
        let plain = ArtifactRequest::new(
            "projects/p/repos/r/revisions/commit",
            "root.fbs",
            SchemaArtifactKind::GeneratedCode,
            Some(Language::Rust),
            &CodegenOptions::default(),
        )
        .expect("plain generated-code request");

        // Act
        let pluggable = ArtifactRequest::new(
            "projects/p/repos/r/revisions/commit",
            "root.fbs",
            SchemaArtifactKind::GeneratedCode,
            Some(Language::Rust),
            &CodegenOptions {
                rust_pluggable_buffer: true,
            },
        )
        .expect("pluggable generated-code request");

        // Assert
        assert_ne!(plain.storage_key, pluggable.storage_key);
        assert_ne!(plain.record, pluggable.record);
    }
}
