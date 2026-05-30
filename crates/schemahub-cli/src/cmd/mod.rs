pub mod branch;
pub mod codegen;
pub mod field;
pub mod history;
pub mod log;
pub mod project;
pub mod repo;
pub mod schema;
pub mod tag;

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
