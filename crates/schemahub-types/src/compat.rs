/// The compatibility direction enforced on a protected branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatibilityDirection {
    Backward,
    Forward,
    Full,
    /// No compatibility checks. For dev/scratch repos.
    Disabled,
}

/// The compatibility configuration applied to a branch mutation.
#[derive(Clone, Debug)]
pub struct CompatibilityRules {
    pub direction: CompatibilityDirection,
}

/// A single compatibility violation found by FormatPlugin::check_compatibility.
#[derive(Clone, Debug)]
pub struct CompatibilityViolation {
    pub declaration_name: String,
    /// Empty if the violation is at the declaration level rather than a specific field.
    pub field_name: Option<String>,
    pub message: String,
}
