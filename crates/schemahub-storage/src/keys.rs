/// Builders for all KV namespace keys.
/// All key formatting logic lives here — no magic strings elsewhere.

use schemahub_types::Hash;

// ── objects/ ─────────────────────────────────────────────────────────────────
// Global, unprefixed. Content-addressed deduplication is inherently cross-repo.

pub fn object_key(hash: &Hash) -> String {
    format!("objects/{}", hash.to_hex())
}

// ── refs/ ─────────────────────────────────────────────────────────────────────

pub fn branch_ref_key(project: &str, repo: &str, branch: &str) -> String {
    format!("refs/{project}/{repo}/heads/{branch}")
}

pub fn tag_ref_key(project: &str, repo: &str, tag: &str) -> String {
    format!("refs/{project}/{repo}/tags/{tag}")
}

pub fn refs_prefix(project: &str, repo: &str) -> String {
    format!("refs/{project}/{repo}/")
}

pub fn branch_refs_prefix(project: &str, repo: &str) -> String {
    format!("refs/{project}/{repo}/heads/")
}

pub fn tag_refs_prefix(project: &str, repo: &str) -> String {
    format!("refs/{project}/{repo}/tags/")
}

// ── deps/ ─────────────────────────────────────────────────────────────────────

pub fn deps_key(project: &str, repo: &str, schema: &str, decl: &str, commit: &str) -> String {
    format!("deps/{project}/{repo}/{schema}/{decl}@{commit}")
}

pub fn deps_prefix(project: &str, repo: &str, schema: &str) -> String {
    format!("deps/{project}/{repo}/{schema}/")
}

// ── index/ ────────────────────────────────────────────────────────────────────

pub fn index_key(project: &str, repo: &str, schema: &str, decl: &str, field: &str) -> String {
    format!("index/{project}/{repo}/{schema}/{decl}/{field}")
}

pub fn index_prefix(project: &str, repo: &str, schema: &str) -> String {
    format!("index/{project}/{repo}/{schema}/")
}

// ── search/ ───────────────────────────────────────────────────────────────────

pub fn search_key(name: &str, project: &str, repo: &str, schema: &str) -> String {
    format!("search/{name}/{project}/{repo}/{schema}")
}

pub fn search_prefix_global(name: &str) -> String {
    format!("search/{name}/")
}

pub fn search_prefix_project(name: &str, project: &str) -> String {
    format!("search/{name}/{project}/")
}

pub fn search_prefix_repo(name: &str, project: &str, repo: &str) -> String {
    format!("search/{name}/{project}/{repo}/")
}

// ── idempotency/ ──────────────────────────────────────────────────────────────

pub fn idempotency_key(project: &str, repo: &str, key: &str) -> String {
    format!("idempotency/{project}/{repo}/{key}")
}

pub fn idempotency_prefix(project: &str, repo: &str) -> String {
    format!("idempotency/{project}/{repo}/")
}

// ── pending/ ──────────────────────────────────────────────────────────────────
// Short-lived GC roots for in-flight mutations.

pub fn pending_key(mutation_id: &str) -> String {
    format!("pending/{mutation_id}")
}

pub const PENDING_PREFIX: &str = "pending/";

// ── roles/ ────────────────────────────────────────────────────────────────────

pub fn role_key(project: &str, identity: &str) -> String {
    format!("roles/{project}/{identity}")
}

pub fn roles_prefix(project: &str) -> String {
    format!("roles/{project}/")
}
