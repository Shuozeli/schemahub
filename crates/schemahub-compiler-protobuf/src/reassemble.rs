//! Reassemble `SchemaObjects` (meta + named decls) back into a
//! `FileDescriptorProto` for printing / codegen.
//!
//! `SchemaObjects::decls` is a `BTreeMap` (sorted by name), so we restore the
//! original *source* declaration order recorded in the meta blob
//! (`message_order` / `enum_order` / `service_order`) before printing — without
//! it, declarations round-trip alphabetized, which is semantically harmless but
//! produces noisy diffs and violates the deterministic-round-trip goal.
//! Declarations not present in the recorded order (e.g. added by a later
//! mutation) are appended in name-sorted order. We still rewrite the top-level
//! `source_code_info` paths so their `[4, i] / [5, i] / [6, i]` prefixes point
//! at the final positions, keeping comment lookup correct across edits.

use std::collections::HashMap;

use protoc_rs_schema::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorProto, ServiceDescriptorProto,
    SourceCodeInfo,
};
use schemahub_types::{PrintError, SchemaObjects};

use crate::blob::{decode_decl, decode_meta, DeclPayload, MetaPayload};
use crate::parse::{FILE_ENUM_TYPE, FILE_MESSAGE_TYPE, FILE_SERVICE};

/// Reassemble `SchemaObjects` into a `FileDescriptorProto`.
pub fn reassemble(schema: &SchemaObjects) -> Result<FileDescriptorProto, PrintError> {
    let meta = decode_meta(schema.meta.as_bytes())
        .map_err(|e| PrintError::MalformedBlob(e.to_string()))?;

    let mut messages: Vec<DescriptorProto> = Vec::new();
    let mut enums: Vec<EnumDescriptorProto> = Vec::new();
    let mut services: Vec<ServiceDescriptorProto> = Vec::new();

    for (name, blob) in &schema.decls {
        let payload =
            decode_decl(blob.as_bytes()).map_err(|e| PrintError::MalformedBlob(e.to_string()))?;
        match payload {
            DeclPayload::Message(m) => {
                debug_assert_eq!(m.name.as_deref().unwrap_or(name), name);
                messages.push(m);
            }
            DeclPayload::Enum(e) => enums.push(e),
            DeclPayload::Service(s) => services.push(s),
        }
    }

    // Restore the recorded source declaration order (BTreeMap iteration above is
    // alphabetical). Unknown names (added post-parse) fall to the end, sorted.
    let messages = reorder_by_name(messages, &meta.message_order, |m| {
        m.name.clone().unwrap_or_default()
    });
    let enums = reorder_by_name(enums, &meta.enum_order, |e| {
        e.name.clone().unwrap_or_default()
    });
    let services = reorder_by_name(services, &meta.service_order, |s| {
        s.name.clone().unwrap_or_default()
    });

    // Rewrite top-level comment paths to the final positions so comment lookup
    // keeps working (identity for unchanged decls now that order is restored).
    let remapped_info = meta
        .source_code_info
        .as_ref()
        .map(|info| remap_source_info(info, &meta, &messages, &enums, &services));

    let file = FileDescriptorProto {
        name: meta.name.clone(),
        package: meta.package.clone(),
        dependency: meta.dependency.clone(),
        public_dependency: meta.public_dependency.clone(),
        weak_dependency: meta.weak_dependency.clone(),
        option_dependency: meta.option_dependency.clone(),
        message_type: messages,
        enum_type: enums,
        service: services,
        extension: meta.extension.clone(),
        options: meta.options.clone(),
        source_code_info: remapped_info,
        syntax: meta.syntax.clone(),
        edition: meta.edition.clone(),
    };

    Ok(file)
}

/// Reorder `items` to match the recorded source `order` (a list of declaration
/// names). Items whose name is in `order` come first, in that order; any
/// remaining items (e.g. added by a mutation after the original parse) are
/// appended in name-sorted order for determinism. An empty `order` (e.g. a blob
/// written before order tracking existed) leaves `items` untouched.
fn reorder_by_name<T>(items: Vec<T>, order: &[String], name_of: impl Fn(&T) -> String) -> Vec<T> {
    if order.is_empty() {
        return items;
    }
    let mut by_name: HashMap<String, T> = items.into_iter().map(|it| (name_of(&it), it)).collect();
    let mut out: Vec<T> = Vec::with_capacity(by_name.len());
    for name in order {
        if let Some(it) = by_name.remove(name) {
            out.push(it);
        }
    }
    let mut rest: Vec<(String, T)> = by_name.into_iter().collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(rest.into_iter().map(|(_, it)| it));
    out
}

/// Build a `path -> SourceLocation` lookup for comment retrieval at print time.
pub fn comment_index(
    info: &SourceCodeInfo,
) -> HashMap<Vec<i32>, &protoc_rs_schema::SourceLocation> {
    let mut map = HashMap::new();
    for loc in &info.location {
        // Keep the first location for a given path (matches descriptor.proto:
        // the first wins for comments).
        map.entry(loc.path.clone()).or_insert(loc);
    }
    map
}

/// Rewrite top-level `[4|5|6, old_idx, ...]` paths so `old_idx` becomes the
/// declaration's new position after the BTreeMap re-sort. The old→new index
/// map is derived from `meta`'s recorded source order and the reassembled
/// (sorted) positions.
fn remap_source_info(
    info: &SourceCodeInfo,
    meta: &MetaPayload,
    messages: &[DescriptorProto],
    enums: &[EnumDescriptorProto],
    services: &[ServiceDescriptorProto],
) -> SourceCodeInfo {
    let msg_map = build_index_map(&meta.message_order, |i| {
        messages.get(i).and_then(|m| m.name.as_deref())
    });
    let enum_map = build_index_map(&meta.enum_order, |i| {
        enums.get(i).and_then(|e| e.name.as_deref())
    });
    let svc_map = build_index_map(&meta.service_order, |i| {
        services.get(i).and_then(|s| s.name.as_deref())
    });

    let mut out = info.clone();
    for loc in &mut out.location {
        if loc.path.len() < 2 {
            continue;
        }
        let kind = loc.path[0];
        let old_idx = loc.path[1];
        let map = match kind {
            FILE_MESSAGE_TYPE => &msg_map,
            FILE_ENUM_TYPE => &enum_map,
            FILE_SERVICE => &svc_map,
            _ => continue,
        };
        if let Some(&new_idx) = map.get(&old_idx) {
            loc.path[1] = new_idx;
        }
    }
    out
}

/// Build `old_index -> new_index`: `order[old]` gives the name, and `new_pos`
/// looks the name up in the reassembled (sorted) list.
fn build_index_map<'a>(
    order: &'a [String],
    new_pos: impl Fn(usize) -> Option<&'a str>,
) -> HashMap<i32, i32> {
    // name -> new index in the reassembled list.
    let mut new_index: HashMap<&str, i32> = HashMap::new();
    let mut i = 0usize;
    while let Some(name) = new_pos(i) {
        new_index.entry(name).or_insert(i as i32);
        i += 1;
    }
    let mut map = HashMap::new();
    for (old_i, name) in order.iter().enumerate() {
        if let Some(&new_i) = new_index.get(name.as_str()) {
            map.insert(old_i as i32, new_i);
        }
    }
    map
}
