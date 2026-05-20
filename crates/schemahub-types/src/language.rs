/// Target language for code generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    Rust,
    Go,
    TypeScript,
    Python,
    Java,
}
