/// The compatibility direction enforced on a protected branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityDirection {
    Backward,
    Forward,
    Full,
    /// No compatibility checks. For dev/scratch repos.
    Disabled,
}

/// The compatibility configuration applied to a bookmark mutation (design.md §7).
#[derive(Clone, Debug)]
pub struct CompatibilityRules {
    pub direction: CompatibilityDirection,
    /// When true, all compatibility checks are skipped.
    pub disabled: bool,
}

impl CompatibilityRules {
    /// Default rules: FULL compatibility, enabled.
    pub fn full() -> Self {
        Self {
            direction: CompatibilityDirection::Full,
            disabled: false,
        }
    }
}

/// A single compatibility violation found by `Compiler::check_compatibility`.
#[derive(Clone, Debug)]
pub struct CompatibilityViolation {
    pub declaration_name: String,
    /// Empty if the violation is at the declaration level rather than a specific field.
    pub field_name: Option<String>,
    pub message: String,
}
