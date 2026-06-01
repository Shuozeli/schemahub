//! Codegen: assemble a `FileDescriptorSet` from a closure and reuse
//! `protoc-rs-codegen` (design.md §10).

use bytes::Bytes;
use protoc_rs_schema::FileDescriptorSet;
use schemahub_types::{CodegenError, DescriptorError, Language, SchemaClosure};

use crate::codec::Writer;
use crate::reassemble::reassemble;

/// Build a `FileDescriptorSet` from every schema in the closure.
fn build_set(closure: &SchemaClosure) -> Result<FileDescriptorSet, String> {
    // Deterministic order by schema path.
    let mut entries: Vec<_> = closure.entries.iter().collect();
    entries.sort_by(|a, b| {
        (&a.0.project, &a.0.repo, &a.0.schema_name).cmp(&(
            &b.0.project,
            &b.0.repo,
            &b.0.schema_name,
        ))
    });

    let mut set = FileDescriptorSet::default();
    for (path, objects) in entries {
        let mut file = reassemble(objects).map_err(|e| e.to_string())?;
        // Ensure the file has a name for codegen module keys.
        if file.name.is_none() {
            file.name = Some(format!(
                "{}/{}/{}",
                path.project, path.repo, path.schema_name
            ));
        }
        set.file.push(file);
    }
    Ok(set)
}

/// Serialize the `FileDescriptorSet` to opaque, versioned bytes.
///
/// `FileDescriptorSet` is not a `prost::Message` in `protoc-rs-schema`, so we
/// serialize each contained file with this crate's stable codec rather than
/// protobuf wire format. These bytes are opaque to the VCS layer.
pub fn generate_descriptors(closure: &SchemaClosure) -> Result<Bytes, DescriptorError> {
    let set = build_set(closure).map_err(DescriptorError::Other)?;
    let mut w = Writer::new();
    w.write_u8(crate::blob::BLOB_VERSION);
    w.write_uvarint(set.file.len() as u64);
    for file in &set.file {
        // Reuse the parse split to encode each file deterministically as
        // meta + decls, concatenated.
        let parsed = crate::parse::split_file(file.clone());
        let meta = parsed.meta.into_bytes();
        w.write_bytes(&meta);
        w.write_uvarint(parsed.decls.len() as u64);
        for (name, blob) in parsed.decls {
            w.write_str(&name);
            w.write_bytes(blob.as_bytes());
        }
    }
    Ok(Bytes::from(w.into_bytes()))
}

/// Generate source code for `lang` over the closure.
pub fn generate_code(closure: &SchemaClosure, lang: Language) -> Result<String, CodegenError> {
    match lang {
        Language::Rust => {
            let set = build_set(closure).map_err(CodegenError::Other)?;
            let modules = protoc_rs_codegen::generate_rust(&set)
                .map_err(|e| CodegenError::Other(e.to_string()))?;
            // Concatenate modules deterministically by key.
            let mut keys: Vec<_> = modules.keys().cloned().collect();
            keys.sort();
            let mut out = String::new();
            for k in keys {
                out.push_str(&format!("// ===== module {k} =====\n"));
                out.push_str(&modules[&k]);
                out.push('\n');
            }
            Ok(out)
        }
        other => Err(CodegenError::UnsupportedLanguage(other)),
    }
}
