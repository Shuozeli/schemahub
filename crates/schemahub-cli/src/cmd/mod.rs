pub mod branch;
pub mod codegen;
pub mod field;
pub mod history;
pub mod log;
pub mod project;
pub mod repo;
pub mod schema;
pub mod tag;

use anyhow::Context;
use tonic::metadata::MetadataValue;
use tonic::Request;

/// Parse a CLI ref string into a `VersionRefKind`:
///   `@<hex>`      → Commit (pinned SHA)
///   `tag:<name>`  → Tag
///   `<name>`      → Branch (default)
pub fn parse_ref(s: &str) -> schemahub_api::schemahub_v1::version_ref::Ref {
    use schemahub_api::schemahub_v1::version_ref::Ref;
    if let Some(sha) = s.strip_prefix('@') {
        Ref::Commit(sha.to_owned())
    } else if let Some(name) = s.strip_prefix("tag:") {
        Ref::Tag(name.to_owned())
    } else {
        Ref::Branch(s.to_owned())
    }
}

/// Wrap a request body in a `tonic::Request` and attach
/// `Authorization: Bearer <token>` when `token` is non-empty.
///
/// Used by every CLI command that talks to an RBAC-enabled server. An
/// empty `token` produces an anonymous request — valid for public-project
/// reads, rejected by the server for writes.
pub fn bearer<T>(body: T, token: &str) -> anyhow::Result<Request<T>> {
    let mut req = Request::new(body);
    if !token.is_empty() {
        let header: MetadataValue<_> = format!("Bearer {token}")
            .parse()
            .context("token contains invalid metadata characters")?;
        req.metadata_mut().insert("authorization", header);
    }
    Ok(req)
}
