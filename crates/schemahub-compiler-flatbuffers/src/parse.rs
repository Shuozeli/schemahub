//! `parse(source)`: run `flatc-rs-parser`, then SPLIT the resulting `Schema`
//! into per-declaration [`DeclBlob`]s plus a file [`MetaBlob`] (design.md §3.2).
//!
//! One `DeclBlob` per `Object` (table/struct), per `Enum` (enum/union), and per
//! `Service`. `Field.id` slot indices are preserved verbatim (wire identity).

use flatc_rs_parser::FbsParser;
use schemahub_types::{DeclBlob, ParseError, ParsedSchema};

use crate::blob::{encode_decl, encode_meta, DeclPayload, FbsMeta};

/// Parse `.fbs` source and split it into addressable objects.
pub fn parse(source: &str) -> Result<ParsedSchema, ParseError> {
    let output = FbsParser::new(source)
        .parse()
        .map_err(|e| ParseError::Other(e.to_string()))?;
    let schema = output.schema;
    let state = output.state;

    // ── Meta ────────────────────────────────────────────────────────────────
    // The active file namespace: the parser stores it per-declaration; take the
    // first declaration that carries one (a single `.fbs` file has at most one
    // active namespace at a time, and schemahub treats one file as one ns scope).
    let namespace = schema
        .objects
        .iter()
        .find_map(|o| o.namespace.as_ref().and_then(|n| n.namespace.clone()))
        .or_else(|| {
            schema
                .enums
                .iter()
                .find_map(|e| e.namespace.as_ref().and_then(|n| n.namespace.clone()))
        })
        .or_else(|| state.root_type_namespace.clone());

    let includes: Vec<String> = schema
        .fbs_files
        .iter()
        .filter_map(|f| f.filename.clone())
        .collect();
    let include_commits = vec![String::new(); includes.len()];

    // Record top-level declaration order across kinds, by source line. The
    // parser splits decls into separate `objects`/`enums`/`services` vectors,
    // so cross-kind interleaving is only recoverable from each decl's span.
    // Decls without a span sort last, then by kind, for a stable fallback.
    let mut order_entries: Vec<(u32, u8, String)> = Vec::new();
    for o in &schema.objects {
        if let Some(n) = &o.name {
            order_entries.push((
                o.span.as_ref().map(|s| s.line).unwrap_or(u32::MAX),
                0,
                n.clone(),
            ));
        }
    }
    for e in &schema.enums {
        if let Some(n) = &e.name {
            order_entries.push((
                e.span.as_ref().map(|s| s.line).unwrap_or(u32::MAX),
                1,
                n.clone(),
            ));
        }
    }
    for s in &schema.services {
        if let Some(n) = &s.name {
            order_entries.push((
                s.span.as_ref().map(|s| s.line).unwrap_or(u32::MAX),
                2,
                n.clone(),
            ));
        }
    }
    order_entries.sort_by_key(|e| (e.0, e.1));
    let decl_order: Vec<String> = order_entries.into_iter().map(|(_, _, n)| n).collect();

    let meta = FbsMeta {
        blob_version: crate::blob::BLOB_VERSION,
        namespace,
        includes,
        include_commits,
        root_type: state.root_type_name.clone(),
        file_ident: schema.file_ident.clone(),
        file_ext: schema.file_ext.clone(),
        decl_order,
        declared_attributes: state.declared_attributes.clone(),
    };

    // ── Declarations (one blob each, in source order) ─────────────────────────
    let mut decls: Vec<(String, DeclBlob)> = Vec::new();

    for obj in schema.objects {
        let name = match &obj.name {
            Some(n) => n.clone(),
            None => continue,
        };
        decls.push((name, encode_decl(DeclPayload::Object(Box::new(obj)))));
    }
    for en in schema.enums {
        let name = match &en.name {
            Some(n) => n.clone(),
            None => continue,
        };
        decls.push((name, encode_decl(DeclPayload::Enum(Box::new(en)))));
    }
    for svc in schema.services {
        let name = match &svc.name {
            Some(n) => n.clone(),
            None => continue,
        };
        decls.push((name, encode_decl(DeclPayload::Service(Box::new(svc)))));
    }

    Ok(ParsedSchema {
        meta: encode_meta(meta),
        decls,
    })
}
