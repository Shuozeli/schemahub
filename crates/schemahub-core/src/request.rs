//! Request / response types — the contract `schemahub-server` is written
//! against. The server maps gRPC messages (schemahub-api) onto these and back
//! (`wire.rs`). Core never touches tonic/prost; these are plain structs.

use schemahub_jj::OpRecord;
use schemahub_types::{DeclSummary, Language, Mutation, SchemaPath};

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
    pub token: Option<String>,
}

/// The result of a successful mutation or transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationResponse {
    pub commit_id: String,
    pub change_id: String,
    /// Declarations that landed conflicted (concurrent same-decl edits) — empty
    /// on a clean write. Never an error (design.md §5.1 concurrency).
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

/// Re-export the op-log record shape so the server can depend on core alone.
pub type OperationRecord = OpRecord;
