//! Canonical `.proto` printer for a reassembled `FileDescriptorProto`
//! (design.md §3.1 — protobuf-rs ships no printer, schemahub owns this).
//!
//! Handles: syntax/edition, package, imports, file options, messages with
//! nested types / oneofs / proto3 `optional` / maps / reserved ranges+names /
//! field options, enums (with reserved), services with streaming RPCs, and
//! leading comments recovered from `source_code_info`.

use std::collections::HashMap;

use protoc_rs_schema::{
    DescriptorProto, EnumDescriptorProto, FieldDescriptorProto, FieldLabel, FieldType,
    FileDescriptorProto, ServiceDescriptorProto, SourceCodeInfo, SourceLocation, Syntax,
};

use crate::parse::{FILE_ENUM_TYPE, FILE_MESSAGE_TYPE, FILE_SERVICE};
use crate::reassemble::comment_index;

// source_code_info field numbers within a DescriptorProto / EnumDescriptorProto.
const MSG_FIELD: i32 = 2;
const MSG_NESTED_TYPE: i32 = 3;
const MSG_ENUM_TYPE: i32 = 4;
const MSG_ONEOF_DECL: i32 = 8;
const ENUM_VALUE: i32 = 2;
const SERVICE_METHOD: i32 = 2;

/// Render a full file to canonical `.proto` source.
pub fn print_file(file: &FileDescriptorProto) -> String {
    let empty = SourceCodeInfo::default();
    let info = file.source_code_info.as_ref().unwrap_or(&empty);
    let comments = comment_index(info);

    let mut out = String::new();

    // syntax / edition
    if let Some(edition) = &file.edition {
        out.push_str(&format!("edition = \"{edition}\";\n\n"));
    } else {
        let syntax = match file.syntax_enum() {
            Syntax::Proto3 => "proto3",
            Syntax::Proto2 => "proto2",
            Syntax::Unknown(_) => "proto2",
        };
        out.push_str(&format!("syntax = \"{syntax}\";\n\n"));
    }

    // package
    if let Some(pkg) = &file.package {
        if !pkg.is_empty() {
            out.push_str(&format!("package {pkg};\n\n"));
        }
    }

    // imports
    if !file.dependency.is_empty() {
        for (i, dep) in file.dependency.iter().enumerate() {
            let idx = i as i32;
            let public = file.public_dependency.contains(&idx);
            let weak = file.weak_dependency.contains(&idx);
            let modifier = if public {
                "public "
            } else if weak {
                "weak "
            } else {
                ""
            };
            out.push_str(&format!("import {modifier}\"{dep}\";\n"));
        }
        out.push('\n');
    }

    // file options
    print_file_options(&mut out, file);

    // Type references are stored fully-qualified (leading `.pkg.Name`); the
    // file's own package is stripped when printing so same-package types stay
    // unqualified (idiomatic), while cross-package types (e.g.
    // `google.protobuf.Timestamp`) keep their qualifier.
    let pkg = file.package.as_deref();

    // The file syntax decides whether singular fields carry an explicit label:
    // proto2 fields are written `optional`/`required`, while proto3/editions
    // singular fields have no label keyword. Editions files (which set `edition`
    // and leave `syntax` unset) are treated like proto3 here, so an editions
    // singular field does not gain a stray `optional`/`required` keyword.
    let is_proto2 = file.edition.is_none() && matches!(file.syntax_enum(), Syntax::Proto2);

    // Emit top-level declarations in original SOURCE order, interleaving
    // messages / enums / services. `FileDescriptorProto` groups them into
    // separate vectors, losing cross-kind order; we recover it from each decl's
    // `source_code_info` span start line. Decls without source info (e.g. added
    // by a later mutation) sort last by line, then by (kind, index) — a stable,
    // deterministic fallback that reduces to message→enum→service.
    enum Top {
        Msg(usize),
        Enum(usize),
        Svc(usize),
    }
    let start_line = |path: &[i32]| -> i64 {
        comments
            .get(path)
            .and_then(|loc| loc.span.first().copied())
            .map(i64::from)
            .unwrap_or(i64::MAX)
    };
    let mut tops: Vec<(i64, i32, usize, Top)> = Vec::new();
    for i in 0..file.message_type.len() {
        tops.push((
            start_line(&[FILE_MESSAGE_TYPE, i as i32]),
            FILE_MESSAGE_TYPE,
            i,
            Top::Msg(i),
        ));
    }
    for i in 0..file.enum_type.len() {
        tops.push((
            start_line(&[FILE_ENUM_TYPE, i as i32]),
            FILE_ENUM_TYPE,
            i,
            Top::Enum(i),
        ));
    }
    for i in 0..file.service.len() {
        tops.push((
            start_line(&[FILE_SERVICE, i as i32]),
            FILE_SERVICE,
            i,
            Top::Svc(i),
        ));
    }
    // Sort by source line, then (kind, index) as a stable tiebreaker.
    tops.sort_by_key(|t| (t.0, t.1, t.2));

    for (_, _, _, top) in &tops {
        match *top {
            Top::Msg(i) => {
                let path = vec![FILE_MESSAGE_TYPE, i as i32];
                print_message(
                    &mut out,
                    &file.message_type[i],
                    0,
                    &path,
                    &comments,
                    pkg,
                    is_proto2,
                );
            }
            Top::Enum(i) => {
                let path = vec![FILE_ENUM_TYPE, i as i32];
                print_enum(&mut out, &file.enum_type[i], 0, &path, &comments);
            }
            Top::Svc(i) => {
                let path = vec![FILE_SERVICE, i as i32];
                print_service(&mut out, &file.service[i], &path, &comments, pkg);
            }
        }
        out.push('\n');
    }

    // File-level `extend <Type> { ... }` blocks (proto2 extensions), grouped by
    // the extended type and emitted in first-seen order.
    print_extend_blocks(&mut out, &file.extension, pkg, is_proto2);

    // Trim trailing blank lines to a single newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

/// Emit `extend <Type> { <fields> }` blocks for file-level extension fields,
/// grouped by their `extendee` (in first-seen order).
fn print_extend_blocks(
    out: &mut String,
    extensions: &[FieldDescriptorProto],
    pkg: Option<&str>,
    is_proto2: bool,
) {
    if extensions.is_empty() {
        return;
    }
    // Preserve first-seen extendee order while grouping.
    let mut order: Vec<&str> = Vec::new();
    for ext in extensions {
        let extendee = ext.extendee.as_deref().unwrap_or("");
        if !order.contains(&extendee) {
            order.push(extendee);
        }
    }
    let no_map_entries: HashMap<&str, (&FieldDescriptorProto, &FieldDescriptorProto)> =
        HashMap::new();
    for extendee in order {
        let ty = rel_type_name(extendee, pkg);
        out.push_str(&format!("extend {ty} {{\n"));
        for ext in extensions
            .iter()
            .filter(|e| e.extendee.as_deref().unwrap_or("") == extendee)
        {
            out.push_str("  ");
            out.push_str(&render_field_body(ext, &no_map_entries, pkg, is_proto2));
            out.push('\n');
        }
        out.push_str("}\n\n");
    }
}

fn print_file_options(out: &mut String, file: &FileDescriptorProto) {
    let Some(opts) = &file.options else {
        return;
    };
    let mut any = false;
    let line = |out: &mut String, k: &str, v: String| {
        out.push_str(&format!("option {k} = {v};\n"));
    };
    if let Some(v) = &opts.java_package {
        line(out, "java_package", quote(v));
        any = true;
    }
    if let Some(v) = &opts.java_outer_classname {
        line(out, "java_outer_classname", quote(v));
        any = true;
    }
    if let Some(v) = opts.java_multiple_files {
        line(out, "java_multiple_files", v.to_string());
        any = true;
    }
    if let Some(v) = &opts.go_package {
        line(out, "go_package", quote(v));
        any = true;
    }
    if let Some(v) = &opts.objc_class_prefix {
        line(out, "objc_class_prefix", quote(v));
        any = true;
    }
    if let Some(v) = &opts.csharp_namespace {
        line(out, "csharp_namespace", quote(v));
        any = true;
    }
    if let Some(v) = &opts.php_namespace {
        line(out, "php_namespace", quote(v));
        any = true;
    }
    if let Some(v) = &opts.ruby_package {
        line(out, "ruby_package", quote(v));
        any = true;
    }
    if let Some(v) = opts.deprecated {
        line(out, "deprecated", v.to_string());
        any = true;
    }
    if let Some(v) = opts.cc_enable_arenas {
        line(out, "cc_enable_arenas", v.to_string());
        any = true;
    }
    if any {
        out.push('\n');
    }
}

fn print_message(
    out: &mut String,
    msg: &DescriptorProto,
    depth: usize,
    path: &[i32],
    comments: &HashMap<Vec<i32>, &SourceLocation>,
    pkg: Option<&str>,
    is_proto2: bool,
) {
    let indent = "  ".repeat(depth);
    emit_comment(out, &indent, path, comments);
    let name = msg.name.as_deref().unwrap_or("");
    out.push_str(&format!("{indent}message {name} {{\n"));

    let inner = "  ".repeat(depth + 1);

    // Message-level options (`option deprecated = true;`, …) appear first in the
    // body. `map_entry` is excluded — it is a synthetic flag the printer already
    // materializes as a `map<K,V>` field, not a user-written option.
    print_message_options(out, msg, &inner);

    // Identify map-entry nested types so we render `map<K,V>` and skip the entry.
    let map_entries: HashMap<&str, (&FieldDescriptorProto, &FieldDescriptorProto)> = msg
        .nested_type
        .iter()
        .filter(|n| {
            n.options
                .as_ref()
                .and_then(|o| o.map_entry)
                .unwrap_or(false)
        })
        .filter_map(|n| {
            let key = n.field.iter().find(|f| f.number == Some(1))?;
            let val = n.field.iter().find(|f| f.number == Some(2))?;
            n.name.as_deref().map(|nm| (nm, (key, val)))
        })
        .collect();

    // proto2 `group` fields: the parser models a group as a field of type Group
    // plus a backing nested message holding the group body. We render the group
    // form (`<label> group <Name> = <n> { ... }`) and skip the backing message
    // from the nested-type loop below. The backing message is matched by the
    // group field's `type_name` (its simple, capitalized name).
    let group_msgs: std::collections::HashSet<&str> = msg
        .field
        .iter()
        .filter(|f| f.r#type == Some(FieldType::Group))
        .filter_map(|f| f.type_name.as_deref().map(short_type_name))
        .collect();

    // Group fields by oneof. Synthetic proto3 `optional` oneofs (proto3_optional)
    // are NOT printed as oneof blocks.
    let mut i = 0;
    while i < msg.field.len() {
        let field = &msg.field[i];
        let field_path = {
            let mut p = path.to_vec();
            p.push(MSG_FIELD);
            p.push(i as i32);
            p
        };

        // proto2 group field: render the group block with its backing message's
        // body, then continue (the backing nested message is skipped below).
        if field.r#type == Some(FieldType::Group) {
            emit_comment(out, &inner, &field_path, comments);
            print_group_field(out, msg, field, depth, pkg, is_proto2);
            i += 1;
            continue;
        }

        let is_real_oneof = field.oneof_index.is_some() && !field.proto3_optional.unwrap_or(false);

        if is_real_oneof {
            let oneof_index = field.oneof_index.unwrap();
            let oneof_name = msg
                .oneof_decl
                .get(oneof_index as usize)
                .and_then(|o| o.name.as_deref())
                .unwrap_or("");
            let oneof_path = {
                let mut p = path.to_vec();
                p.push(MSG_ONEOF_DECL);
                p.push(oneof_index);
                p
            };
            emit_comment(out, &inner, &oneof_path, comments);
            out.push_str(&format!("{inner}oneof {oneof_name} {{\n"));
            let member_indent = "  ".repeat(depth + 2);
            // Emit all consecutive fields that belong to this oneof.
            while i < msg.field.len() {
                let f = &msg.field[i];
                if f.oneof_index != Some(oneof_index) || f.proto3_optional.unwrap_or(false) {
                    break;
                }
                let fp = {
                    let mut p = path.to_vec();
                    p.push(MSG_FIELD);
                    p.push(i as i32);
                    p
                };
                emit_comment(out, &member_indent, &fp, comments);
                out.push_str(&member_indent);
                out.push_str(&render_field_body(f, &map_entries, pkg, is_proto2));
                out.push_str(&trailing_comment_suffix(&fp, comments));
                out.push('\n');
                i += 1;
            }
            out.push_str(&format!("{inner}}}\n"));
            continue;
        }

        // Regular / repeated / proto3-optional / map field.
        emit_comment(out, &inner, &field_path, comments);
        out.push_str(&inner);
        out.push_str(&render_field(field, &map_entries, pkg, is_proto2));
        out.push_str(&trailing_comment_suffix(&field_path, comments));
        out.push('\n');
        i += 1;
    }

    // Nested enums.
    for (j, en) in msg.enum_type.iter().enumerate() {
        let mut p = path.to_vec();
        p.push(MSG_ENUM_TYPE);
        p.push(j as i32);
        print_enum(out, en, depth + 1, &p, comments);
    }

    // Nested messages (skipping synthetic map-entry types and group bodies).
    for (j, nested) in msg.nested_type.iter().enumerate() {
        let is_map_entry = nested
            .options
            .as_ref()
            .and_then(|o| o.map_entry)
            .unwrap_or(false);
        if is_map_entry {
            continue;
        }
        // A group field's backing message is printed inline as the group body.
        if nested
            .name
            .as_deref()
            .is_some_and(|n| group_msgs.contains(n))
        {
            continue;
        }
        let mut p = path.to_vec();
        p.push(MSG_NESTED_TYPE);
        p.push(j as i32);
        print_message(out, nested, depth + 1, &p, comments, pkg, is_proto2);
    }

    // extensions ranges (proto2): `extensions <start> to <end>;`.
    for r in &msg.extension_range {
        let (start, end) = (r.start.unwrap_or(0), r.end.unwrap_or(0));
        // ExtensionRange end is exclusive; `end - 1` is the last extendable
        // number. The protobuf max field number sentinel reparses as `max`.
        let last = end - 1;
        if last <= start {
            out.push_str(&format!("{inner}extensions {start};\n"));
        } else {
            out.push_str(&format!("{inner}extensions {start} to {last};\n"));
        }
    }

    // reserved ranges
    for r in &msg.reserved_range {
        let (start, end) = (r.start.unwrap_or(0), r.end.unwrap_or(0));
        out.push_str(&inner);
        // ReservedRange end is exclusive; `end - 1` is the last reserved number.
        if end - start <= 1 {
            out.push_str(&format!("reserved {start};\n"));
        } else {
            out.push_str(&format!("reserved {start} to {};\n", end - 1));
        }
    }
    // reserved names
    if !msg.reserved_name.is_empty() {
        let names: Vec<String> = msg.reserved_name.iter().map(|n| quote(n)).collect();
        out.push_str(&format!("{inner}reserved {};\n", names.join(", ")));
    }

    out.push_str(&format!("{indent}}}\n"));
}

/// Render a proto2 `group` field: `<label> group <Name> = <number> { <body> }`.
/// The body fields come from the group's backing nested message (matched by the
/// field's `type_name`); that message is skipped from the normal nested-type
/// output. The group's declared name is the (capitalized) message name, not the
/// lowercased implicit field name.
fn print_group_field(
    out: &mut String,
    parent: &DescriptorProto,
    field: &FieldDescriptorProto,
    depth: usize,
    pkg: Option<&str>,
    is_proto2: bool,
) {
    let indent = "  ".repeat(depth + 1);
    let group_name = field
        .type_name
        .as_deref()
        .map(short_type_name)
        .unwrap_or("");
    let number = field.number.unwrap_or(0);
    let label = match field.label {
        Some(FieldLabel::Repeated) => "repeated ",
        Some(FieldLabel::Required) => "required ",
        Some(FieldLabel::Optional) if is_proto2 => "optional ",
        _ => "",
    };
    out.push_str(&format!(
        "{indent}{label}group {group_name} = {number} {{\n"
    ));

    // Find the backing message and print its body (fields, then nested decls).
    if let Some(body) = parent
        .nested_type
        .iter()
        .find(|n| n.name.as_deref() == Some(group_name))
    {
        let body_indent = "  ".repeat(depth + 2);
        let no_map_entries: HashMap<&str, (&FieldDescriptorProto, &FieldDescriptorProto)> =
            HashMap::new();
        for f in &body.field {
            out.push_str(&body_indent);
            out.push_str(&render_field_body(f, &no_map_entries, pkg, is_proto2));
            out.push('\n');
        }
    }
    out.push_str(&format!("{indent}}}\n"));
}

/// Emit message-level options (`option <name> = <value>;`) for the set bool
/// fields of `MessageOptions`, excluding the synthetic `map_entry` flag.
fn print_message_options(out: &mut String, msg: &DescriptorProto, inner: &str) {
    let Some(opts) = &msg.options else {
        return;
    };
    if let Some(v) = opts.message_set_wire_format {
        out.push_str(&format!("{inner}option message_set_wire_format = {v};\n"));
    }
    if let Some(v) = opts.no_standard_descriptor_accessor {
        out.push_str(&format!(
            "{inner}option no_standard_descriptor_accessor = {v};\n"
        ));
    }
    if let Some(v) = opts.deprecated {
        out.push_str(&format!("{inner}option deprecated = {v};\n"));
    }
    // `map_entry` is intentionally NOT emitted: it marks a synthetic map-entry
    // type, which the printer renders as a `map<K,V>` field instead.
}

/// Render a field with its trailing newline-less body, choosing map vs scalar.
fn render_field(
    field: &FieldDescriptorProto,
    map_entries: &HashMap<&str, (&FieldDescriptorProto, &FieldDescriptorProto)>,
    pkg: Option<&str>,
    is_proto2: bool,
) -> String {
    render_field_body(field, map_entries, pkg, is_proto2)
}

fn render_field_body(
    field: &FieldDescriptorProto,
    map_entries: &HashMap<&str, (&FieldDescriptorProto, &FieldDescriptorProto)>,
    pkg: Option<&str>,
    is_proto2: bool,
) -> String {
    let name = field.name.as_deref().unwrap_or("");
    let number = field.number.unwrap_or(0);

    // Map field: a repeated message whose type is a map-entry nested type.
    if field.label == Some(FieldLabel::Repeated) && field.r#type == Some(FieldType::Message) {
        if let Some(type_name) = field.type_name.as_deref() {
            let short = short_type_name(type_name);
            if let Some((key, val)) = map_entries.get(short) {
                let k = type_str(key, pkg);
                let v = type_str(val, pkg);
                return format!(
                    "map<{k}, {v}> {name} = {number}{};",
                    field_options_str(field)
                );
            }
        }
    }

    let mut prefix = String::new();
    if field.proto3_optional.unwrap_or(false) {
        prefix.push_str("optional ");
    } else {
        match field.label {
            Some(FieldLabel::Repeated) => prefix.push_str("repeated "),
            Some(FieldLabel::Required) => prefix.push_str("required "),
            // proto2 singular fields are written with an explicit `optional`
            // label; proto3/editions singular fields carry no label keyword.
            // Oneof members never take a label.
            Some(FieldLabel::Optional) if is_proto2 && field.oneof_index.is_none() => {
                prefix.push_str("optional ");
            }
            _ => {}
        }
    }

    let ty = type_str(field, pkg);
    format!(
        "{prefix}{ty} {name} = {number}{};",
        field_options_str(field)
    )
}

/// The `.proto` type string for a field.
fn type_str(field: &FieldDescriptorProto, pkg: Option<&str>) -> String {
    match field.r#type {
        Some(FieldType::Message) | Some(FieldType::Enum) | Some(FieldType::Group) => field
            .type_name
            .as_deref()
            .map(|n| rel_type_name(n, pkg))
            .unwrap_or("")
            .to_string(),
        Some(t) => t.proto_name().to_string(),
        None => field
            .type_name
            .as_deref()
            .map(|n| rel_type_name(n, pkg))
            .unwrap_or("")
            .to_string(),
    }
}

/// Render a stored fully-qualified type reference (`.pkg.Name` /
/// `.google.protobuf.Timestamp`) relative to the file's own package. The leading
/// dot is always dropped; the file's own package prefix is stripped so
/// same-package types print unqualified (idiomatic). Cross-package references
/// keep their qualifier so the reconstructed source still resolves — dropping it
/// (the old behavior) turned `google.protobuf.Timestamp` into an unresolvable
/// `Timestamp`.
fn rel_type_name<'a>(name: &'a str, pkg: Option<&str>) -> &'a str {
    let trimmed = name.strip_prefix('.').unwrap_or(name);
    if let Some(p) = pkg.filter(|p| !p.is_empty()) {
        if let Some(rest) = trimmed.strip_prefix(p) {
            if let Some(rest) = rest.strip_prefix('.') {
                return rest;
            }
        }
    }
    trimmed
}

/// Strip a qualifier to the trailing type name. Used only for keying local
/// nested map-entry types (always same-file), never for printed type references.
fn short_type_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Render the inline `[...]` option list for a field, composing the proto2
/// `default` value with the `FieldOptions` entries (`deprecated`, `packed`,
/// `lazy`, `json_name`). Returns `""` when nothing is set, else ` [a, b, ...]`.
fn field_options_str(field: &FieldDescriptorProto) -> String {
    let mut parts = Vec::new();

    // `default = <value>` (proto2). It is carried in `FieldDescriptorProto`,
    // not `FieldOptions`. String/bytes defaults are quoted (incl. unicode);
    // numeric / bool / enum-value defaults are emitted as written.
    if let Some(default) = &field.default_value {
        let rendered = match field.r#type {
            Some(FieldType::String) | Some(FieldType::Bytes) => quote(default),
            _ => default.clone(),
        };
        parts.push(format!("default = {rendered}"));
    }

    if let Some(opts) = &field.options {
        if let Some(true) = opts.deprecated {
            parts.push("deprecated = true".to_string());
        }
        if let Some(packed) = opts.packed {
            parts.push(format!("packed = {packed}"));
        }
        if let Some(true) = opts.lazy {
            parts.push("lazy = true".to_string());
        }
    }

    // `json_name` is a field-level option (stored directly on the field, not in
    // `FieldOptions`). The parser always populates it with the auto-derived
    // camelCase default, so only emit it when it was set EXPLICITLY — i.e. it
    // differs from the camelCase of the field name. It composes with the sibling
    // options in one `[...]`.
    if let (Some(json_name), Some(name)) = (&field.json_name, &field.name) {
        if *json_name != protoc_rs_parser::to_camel_case(name) {
            parts.push(format!("json_name = {}", quote(json_name)));
        }
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
    }
}

fn print_enum(
    out: &mut String,
    en: &EnumDescriptorProto,
    depth: usize,
    path: &[i32],
    comments: &HashMap<Vec<i32>, &SourceLocation>,
) {
    let indent = "  ".repeat(depth);
    emit_comment(out, &indent, path, comments);
    let name = en.name.as_deref().unwrap_or("");
    out.push_str(&format!("{indent}enum {name} {{\n"));
    let inner = "  ".repeat(depth + 1);

    // Enum-level options. `allow_alias` is required to print before aliased
    // values so the reparse accepts duplicate value numbers; `deprecated` (and
    // any future bool option) is emitted alongside it.
    if let Some(opts) = &en.options {
        if opts.allow_alias.unwrap_or(false) {
            out.push_str(&format!("{inner}option allow_alias = true;\n"));
        }
        if let Some(v) = opts.deprecated {
            out.push_str(&format!("{inner}option deprecated = {v};\n"));
        }
    }

    for (i, val) in en.value.iter().enumerate() {
        let mut p = path.to_vec();
        p.push(ENUM_VALUE);
        p.push(i as i32);
        emit_comment(out, &inner, &p, comments);
        let vn = val.name.as_deref().unwrap_or("");
        let num = val.number.unwrap_or(0);
        out.push_str(&format!("{inner}{vn} = {num};\n"));
    }

    for r in &en.reserved_range {
        let (start, end) = (r.start.unwrap_or(0), r.end.unwrap_or(0));
        if start == end {
            out.push_str(&format!("{inner}reserved {start};\n"));
        } else {
            out.push_str(&format!("{inner}reserved {start} to {end};\n"));
        }
    }
    if !en.reserved_name.is_empty() {
        let names: Vec<String> = en.reserved_name.iter().map(|n| quote(n)).collect();
        out.push_str(&format!("{inner}reserved {};\n", names.join(", ")));
    }

    out.push_str(&format!("{indent}}}\n"));
}

fn print_service(
    out: &mut String,
    svc: &ServiceDescriptorProto,
    path: &[i32],
    comments: &HashMap<Vec<i32>, &SourceLocation>,
    pkg: Option<&str>,
) {
    emit_comment(out, "", path, comments);
    let name = svc.name.as_deref().unwrap_or("");
    out.push_str(&format!("service {name} {{\n"));

    for (i, m) in svc.method.iter().enumerate() {
        let mut p = path.to_vec();
        p.push(SERVICE_METHOD);
        p.push(i as i32);
        emit_comment(out, "  ", &p, comments);
        let mn = m.name.as_deref().unwrap_or("");
        let input = rel_type_name(m.input_type.as_deref().unwrap_or(""), pkg);
        let output = rel_type_name(m.output_type.as_deref().unwrap_or(""), pkg);
        let client_stream = if m.client_streaming.unwrap_or(false) {
            "stream "
        } else {
            ""
        };
        let server_stream = if m.server_streaming.unwrap_or(false) {
            "stream "
        } else {
            ""
        };
        out.push_str(&format!(
            "  rpc {mn}({client_stream}{input}) returns ({server_stream}{output});\n"
        ));
    }

    out.push_str("}\n");
}

/// Emit any detached comment blocks and the leading comment (if any) for `path`,
/// one `// ...` line per source line. Detached comments are emitted first, each
/// as its own block separated by a blank line, mirroring the source layout that
/// detaches them from the declaration.
fn emit_comment(
    out: &mut String,
    indent: &str,
    path: &[i32],
    comments: &HashMap<Vec<i32>, &SourceLocation>,
) {
    if let Some(loc) = comments.get(path) {
        for detached in &loc.leading_detached_comments {
            emit_comment_block(out, indent, detached);
            // A blank line keeps the detached comment detached from what follows.
            out.push('\n');
        }
        if let Some(text) = &loc.leading_comments {
            emit_comment_block(out, indent, text);
        }
    }
}

/// Emit a multi-line comment `text` as `// ...` lines at `indent`.
fn emit_comment_block(out: &mut String, indent: &str, text: &str) {
    for line in text.trim_end_matches('\n').lines() {
        out.push_str(indent);
        out.push_str("//");
        if !line.is_empty() && !line.starts_with(' ') {
            out.push(' ');
        }
        out.push_str(line);
        out.push('\n');
    }
}

/// The trailing comment for `path` rendered as an inline ` // ...` suffix to be
/// appended after the statement on the same line, or `""` when there is none.
/// Newlines inside a trailing comment are collapsed to spaces so the suffix
/// stays on one physical line and reparses cleanly.
fn trailing_comment_suffix(path: &[i32], comments: &HashMap<Vec<i32>, &SourceLocation>) -> String {
    let Some(loc) = comments.get(path) else {
        return String::new();
    };
    let Some(text) = &loc.trailing_comments else {
        return String::new();
    };
    let collapsed: String = text
        .trim_end_matches('\n')
        .lines()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        return String::new();
    }
    let sep = if collapsed.starts_with(' ') { "" } else { " " };
    format!(" //{sep}{collapsed}")
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
