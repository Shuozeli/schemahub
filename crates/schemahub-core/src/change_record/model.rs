use schemahub_types::{Identity, IdentityKind, SchemaPath};
use serde::{Deserialize, Serialize};

/// Server-derived actor metadata stored with a change record or review.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeActor {
    pub identity: String,
    pub kind: IdentityKind,
    pub display_name: Option<String>,
    pub delegated_by: Option<String>,
}

impl From<&Identity> for ChangeActor {
    fn from(identity: &Identity) -> Self {
        Self {
            identity: identity.id().unwrap_or_default().to_string(),
            kind: identity.kind(),
            display_name: identity.display().map(str::to_string),
            delegated_by: identity.delegated_by().map(str::to_string),
        }
    }
}

/// One executable edit in a change record.
///
/// Compiler-specific mutation bytes retain their format discriminator and are
/// decoded only by the selected compiler. Full-source and delete operations are
/// explicit so the ledger can represent every schema lifecycle workflow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChangeEdit {
    Mutation {
        schema: SchemaPath,
        format_id: String,
        operation: Vec<u8>,
    },
    ReplaceSource {
        schema: SchemaPath,
        format_id: String,
        source: String,
    },
    DeleteSchema {
        schema: SchemaPath,
        format_id: String,
    },
}

/// Externally visible lifecycle state. `Applying` is server-managed and
/// recoverable after a crash; terminal records are immutable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeRecordStatus {
    Draft,
    Ready,
    Applying,
    Applied,
    Rejected,
    Abandoned,
}

impl ChangeRecordStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Applied | Self::Rejected | Self::Abandoned)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub code: String,
    pub message: String,
    pub schema_name: Option<String>,
    pub declaration_name: Option<String>,
}

/// Stored output of validating the record's current target, base, and edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub resolved_base_commit: String,
    pub edit_digest: String,
    pub issues: Vec<ValidationIssue>,
    pub validated_at_unix_ms: i64,
    pub validator_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReviewDecision {
    Approved,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeReview {
    pub reviewer: ChangeActor,
    pub decision: ChangeReviewDecision,
    pub reason: String,
    pub create_time_unix_ms: i64,
}

/// Immutable receipt that links an applied record to JJ and the serving plane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyResult {
    pub commit_id: String,
    pub change_id: String,
    pub operation_id: String,
    pub conflicted_declarations: Vec<String>,
    pub artifact_digest: Option<String>,
}

/// Durable ownership/correlation state for a recoverable Apply attempt.
/// The attempt is retained after success so retries with the same request id
/// can return the original receipt without creating another commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyAttempt {
    pub request_id: String,
    pub attempt_id: String,
    pub actor: ChangeActor,
    pub lease_owner: String,
    pub lease_expires_at_unix_ms: i64,
    pub start_time_unix_ms: i64,
    pub update_time_unix_ms: i64,
}

/// Durable schema-change intent and its lifecycle outputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub name: String,
    pub project: String,
    pub repo: String,
    pub target_bookmark: String,
    pub base_revision: Option<String>,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub external_references: Vec<String>,
    pub edits: Vec<ChangeEdit>,
    pub created_by: ChangeActor,
    pub status: ChangeRecordStatus,
    pub validation: Option<ValidationResult>,
    pub reviews: Vec<ChangeReview>,
    #[serde(default)]
    pub apply_attempt: Option<ApplyAttempt>,
    pub apply_result: Option<ApplyResult>,
    pub etag: String,
    pub create_time_unix_ms: i64,
    pub update_time_unix_ms: i64,
}

/// Input fields accepted by Create. Output-only audit and lifecycle fields are
/// intentionally absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateChange {
    pub project: String,
    pub repo: String,
    pub change_id: Option<String>,
    pub target_bookmark: String,
    pub base_revision: Option<String>,
    pub title: String,
    pub description: String,
    pub external_references: Vec<String>,
    pub edits: Vec<ChangeEdit>,
}

/// Field-mask-shaped patch used by Update. `None` means leave the field alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeUpdate {
    pub target_bookmark: Option<String>,
    pub base_revision: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub external_references: Option<Vec<String>>,
    pub edits: Option<Vec<ChangeEdit>>,
}

impl ChangeUpdate {
    pub fn is_empty(&self) -> bool {
        self.target_bookmark.is_none()
            && self.base_revision.is_none()
            && self.title.is_none()
            && self.description.is_none()
            && self.external_references.is_none()
            && self.edits.is_none()
    }
}
