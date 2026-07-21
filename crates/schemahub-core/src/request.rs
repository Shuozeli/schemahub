//! Request / response types — the contract `schemahub-server` is written
//! against. The server maps gRPC messages (schemahub-api) onto these and back
//! (`wire.rs`). Core never touches tonic/prost; these are plain structs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use schemahub_jj::OpRecord;
use schemahub_types::{
    CodegenOptions, DeclChange, DeclDetail, DeclSummary, Import, Language, Mutation, SchemaPath,
};

/// Create a whole schema file from source text.
///
/// The explicit `format_id` is part of the public create contract and must
/// agree with the schema-name extension. The core enforces that relationship
/// so transport adapters cannot bypass it.
#[derive(Clone, Debug)]
pub struct CreateSchemaRequest {
    pub schema: SchemaPath,
    pub bookmark: String,
    pub format_id: String,
    pub source: String,
    pub author: String,
    pub message: String,
    pub idempotency_key: Option<String>,
    pub base_revision: Option<String>,
    pub token: Option<String>,
}

/// Replace an existing schema file with a complete source document.
#[derive(Clone, Debug)]
pub struct UpdateSchemaRequest {
    pub schema: SchemaPath,
    pub bookmark: String,
    pub source: String,
    pub author: String,
    pub message: String,
    pub force: bool,
    pub idempotency_key: Option<String>,
    pub base_revision: Option<String>,
    pub token: Option<String>,
}

/// Delete an existing schema file.
///
/// `force` bypasses protected-bookmark compatibility policy and therefore
/// requires the `Force` authorization action. It never bypasses live-reference
/// integrity.
#[derive(Clone, Debug)]
pub struct DeleteSchemaRequest {
    pub schema: SchemaPath,
    pub bookmark: String,
    pub author: String,
    pub message: String,
    pub force: bool,
    pub idempotency_key: Option<String>,
    pub base_revision: Option<String>,
    pub token: Option<String>,
}

/// A single-mutation request (design.md §5.1).
#[derive(Clone, Debug)]
pub struct MutationRequest {
    /// The bookmark to mutate (e.g. "main", "feature/x").
    pub bookmark: String,
    /// The typed-op envelope. Its `schema_path` + `format_id` route the mutation.
    pub mutation: Mutation,
    /// Commit author identity string (recorded in the op-log).
    pub author: String,
    /// Commit message.
    pub message: String,
    /// `--force`: skip the compatibility gate on protected bookmarks. Requires
    /// the `Force` authz action.
    pub force: bool,
    /// RPC-edge idempotency key. Literal retries with the same key return the
    /// stored result (design.md §5.1 step 1).
    pub idempotency_key: Option<String>,
    /// Optional retained commit proving the caller's causal base. This is
    /// validated for repository ownership but is not a branch-head CAS gate.
    pub base_revision: Option<String>,
    /// Optional auth token from request metadata (passed to `AuthnProvider`).
    pub token: Option<String>,
}

/// A transaction request: an ordered batch applied under one commit / one
/// operation (design.md §5.2).
#[derive(Clone, Debug)]
pub struct TransactionRequest {
    pub bookmark: String,
    /// The ordered ops. Every op must share one `(project, repo)` + `format_id`;
    /// ops may target different schema files within that repo — the whole batch
    /// lands atomically in one commit (up to `TransactionLimits::max_schemas`).
    pub mutations: Vec<Mutation>,
    pub author: String,
    pub message: String,
    pub force: bool,
    pub idempotency_key: Option<String>,
    pub base_revision: Option<String>,
    pub token: Option<String>,
}

/// Monotonic server deadline plus cooperative cancellation for one transaction.
///
/// The transport owns the timer and cancels the shared token when it returns a
/// deadline error. Core checks both the absolute instant and the token during
/// planning and again inside the final publication callback, so detached
/// blocking work cannot begin publication after the server has timed out.
#[derive(Clone, Debug)]
pub struct TransactionDeadline {
    expires_at: Instant,
    cancelled: Arc<AtomicBool>,
}

impl TransactionDeadline {
    pub fn after(timeout: Duration) -> Self {
        let now = Instant::now();
        Self {
            // An unrepresentable duration fails closed as already expired.
            expires_at: now.checked_add(timeout).unwrap_or(now),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_exceeded(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.expires_at
    }
}

/// The result of a successful mutation or transaction.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MutationResponse {
    pub commit_id: String,
    pub change_id: String,
    /// Declarations that landed conflicted on an unprotected bookmark. A
    /// protected bookmark rejects the exact conflicted final tree before JJ
    /// publication, so successful protected writes always return an empty list.
    pub conflicted_decls: Vec<String>,
}

/// Limits enforced before a transaction is applied (design.md §5.2).
#[derive(Clone, Copy, Debug)]
pub struct TransactionLimits {
    pub max_ops: usize,
    pub max_schemas: usize,
}

impl Default for TransactionLimits {
    fn default() -> Self {
        Self {
            max_ops: 100,
            // A single transaction may touch up to 20 schema files atomically
            // (design.md §3.4).
            max_schemas: 20,
        }
    }
}

/// A codegen request (design.md §10).
#[derive(Clone, Debug)]
pub struct CodegenRequest {
    pub schema: SchemaPath,
    pub bookmark: String,
    pub lang: Language,
    pub options: CodegenOptions,
}

// ── Exploration / history response shapes ───────────────────────────────────

/// One entry in the commit/change history graph (design.md §12 `log`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub commit_id: String,
    pub change_id: String,
    pub parents: Vec<String>,
    pub author: String,
    pub message: String,
    pub timestamp: String,
}

/// Semantic repository diff together with the exact immutable snapshots used
/// on both sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryDiff {
    pub schema_diffs: Vec<(String, Vec<DeclChange>)>,
    pub base_commit: String,
    pub head_commit: String,
}

/// A search hit: where a matching declaration lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub schema_name: String,
    pub decl_name: String,
}

/// A declaration summary tagged with the schema file it came from (Search).
#[derive(Clone, Debug)]
pub struct DeclLocation {
    pub schema_name: String,
    pub summary: DeclSummary,
}

/// Fully resolved result of following one field/property's named type.
#[derive(Clone, Debug)]
pub struct FollowedType {
    pub source_commit: String,
    pub target_schema: SchemaPath,
    pub target_commit: String,
    pub summary: DeclSummary,
    pub detail: DeclDetail,
    pub pinned: bool,
    pub import_path: String,
}

/// One forward import edge with normalized endpoints and explicit resolution
/// state. The raw compiler import remains available for exact source fidelity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaDependency {
    pub importing_schema: SchemaPath,
    pub importing_commit: String,
    pub imported_schema: SchemaPath,
    pub target_commit: String,
    pub resolved: bool,
    pub import: Import,
}

/// The immutable default-bookmark snapshot inspected for one repository by a
/// bounded cross-repository reverse-dependency scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependencyScanSnapshot {
    pub project: String,
    pub repo: String,
    pub bookmark: String,
    pub commit_id: String,
}

/// One direct import edge from a visible repository to the queried schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaDependent {
    pub importing_schema: SchemaPath,
    pub importing_bookmark: String,
    pub importing_commit: String,
    pub import: Import,
}

/// Complete successful result of one bounded, visibility-filtered scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependentsScan {
    pub dependents: Vec<SchemaDependent>,
    pub snapshots: Vec<DependencyScanSnapshot>,
    pub schemas_scanned: usize,
}

/// Re-export the op-log record shape so the server can depend on core alone.
pub type OperationRecord = OpRecord;
