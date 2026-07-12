/// Format-specific code generation options.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodegenOptions {
    /// For FlatBuffers Rust output, generate readers over the sibling
    /// `flatc-rs-codegen` pluggable byte-buffer abstraction.
    pub rust_pluggable_buffer: bool,
}
