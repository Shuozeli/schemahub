//! First-class conflict representation at declaration granularity (design.md §6).

use crate::blob::DeclBlob;

/// The N competing sides of a conflicted declaration, as handed to the compiler
/// for rendering. `base` is the common ancestor (absent for add/add conflicts);
/// `sides` are the divergent versions (e.g. `ours`, `theirs`).
#[derive(Clone, Debug, Default)]
pub struct ConflictSides {
    pub base: Option<DeclBlob>,
    pub sides: Vec<DeclBlob>,
}

impl ConflictSides {
    pub fn new(base: Option<DeclBlob>, sides: Vec<DeclBlob>) -> Self {
        Self { base, sides }
    }
}
