//! The objects produced by parsing and consumed by printing/codegen (design.md §2).

use std::collections::{BTreeMap, HashMap};

use crate::blob::{DeclBlob, MetaBlob};
use crate::schema_path::SchemaPath;

/// Output of [`Compiler::parse`](crate::compiler::Compiler::parse): the file
/// split into addressable objects.
#[derive(Clone, Debug, Default)]
pub struct ParsedSchema {
    /// File-level metadata: package, imports, syntax/edition.
    pub meta: MetaBlob,
    /// `(declaration name, serialized AST node)`, in source order.
    pub decls: Vec<(String, DeclBlob)>,
}

/// A schema loaded from storage for mutation/printing: meta + named decls.
///
/// `decls` is a `BTreeMap` so iteration is deterministic for canonical printing.
#[derive(Clone, Debug, Default)]
pub struct SchemaObjects {
    pub meta: MetaBlob,
    pub decls: BTreeMap<String, DeclBlob>,
}

impl SchemaObjects {
    pub fn new(meta: MetaBlob) -> Self {
        Self {
            meta,
            decls: BTreeMap::new(),
        }
    }
}

/// The transitive import closure handed to a compiler for codegen (design.md §10).
///
/// Each entry is one schema file's reassembled objects, keyed by its full path.
#[derive(Clone, Debug, Default)]
pub struct SchemaClosure {
    pub entries: HashMap<SchemaPath, SchemaObjects>,
}

impl SchemaClosure {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}
