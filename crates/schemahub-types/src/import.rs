/// An import of another SchemaHub schema.
/// Stored inside AST blobs wherever one schema references another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// Logical path: "project/repo/schema-file-name"
    pub path: String,
    /// Pinned commit hash, or empty for a live import resolved under Core's
    /// immutable snapshot rules.
    pub resolved_commit: String,
    /// The declaration name within the imported schema.
    pub decl_name: String,
}
