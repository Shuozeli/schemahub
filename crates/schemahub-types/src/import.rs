/// A pinned import of an external schemahub schema.
/// Stored inside AST blobs wherever one schema references another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// Logical path: "project/repo/schema-file-name"
    pub path: String,
    /// Pinned commit hash at the time this import was added or last updated.
    pub resolved_commit: String,
    /// The declaration name within the imported schema.
    pub decl_name: String,
}
