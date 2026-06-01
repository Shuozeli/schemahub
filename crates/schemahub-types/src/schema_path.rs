use std::fmt;

/// Identifies a schema file within the registry.
/// Corresponds to the project / repo / schema namespacing from the requirements.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SchemaPath {
    pub project: String,
    pub repo: String,
    pub schema_name: String,
}

impl SchemaPath {
    pub fn new(
        project: impl Into<String>,
        repo: impl Into<String>,
        schema_name: impl Into<String>,
    ) -> Self {
        Self {
            project: project.into(),
            repo: repo.into(),
            schema_name: schema_name.into(),
        }
    }
}

impl fmt::Display for SchemaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.project, self.repo, self.schema_name)
    }
}

impl fmt::Debug for SchemaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SchemaPath({})", self)
    }
}
