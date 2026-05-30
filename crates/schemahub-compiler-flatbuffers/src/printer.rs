//! Canonical `.fbs` printer (design.md §3.2). `flatbuffers-rs` ships no printer,
//! so schemahub owns it. The printer is deterministic and round-trip-faithful:
//! `parse → split → reassemble → print → parse` yields an equivalent `Schema`.
//!
//! Declaration order, attributes, `deprecated`, and documentation are preserved.

use flatc_rs_schema::{
    Attributes, BaseType, Documentation, Enum, EnumVal, Field, Object, RpcCall, Service, Type,
};

use crate::blob::{DeclPayload, FbsMeta};

/// Render the canonical FlatBuffers source for a base scalar type.
///
/// Uses the canonical (non-aliased) spelling: `int8`/`uint8`/... are normalized
/// to `byte`/`ubyte`/... matching the official `flatc` reflection vocabulary.
fn base_type_name(bt: BaseType) -> &'static str {
    match bt {
        BaseType::BASE_TYPE_BOOL => "bool",
        BaseType::BASE_TYPE_BYTE => "byte",
        BaseType::BASE_TYPE_U_BYTE => "ubyte",
        BaseType::BASE_TYPE_SHORT => "short",
        BaseType::BASE_TYPE_U_SHORT => "ushort",
        BaseType::BASE_TYPE_INT => "int",
        BaseType::BASE_TYPE_U_INT => "uint",
        BaseType::BASE_TYPE_LONG => "long",
        BaseType::BASE_TYPE_U_LONG => "ulong",
        BaseType::BASE_TYPE_FLOAT => "float",
        BaseType::BASE_TYPE_DOUBLE => "double",
        BaseType::BASE_TYPE_STRING => "string",
        BaseType::BASE_TYPE_U_TYPE => "uint8",
        // Aggregate / non-leaf types are rendered via their referenced name.
        BaseType::BASE_TYPE_NONE
        | BaseType::BASE_TYPE_VECTOR
        | BaseType::BASE_TYPE_VECTOR64
        | BaseType::BASE_TYPE_TABLE
        | BaseType::BASE_TYPE_STRUCT
        | BaseType::BASE_TYPE_UNION
        | BaseType::BASE_TYPE_ARRAY => "",
    }
}

/// Render a field's type to its canonical `.fbs` spelling.
///
/// Scalars print as their keyword; user-defined types print as their
/// `unresolved_name`; vectors as `[elem]`; fixed arrays as `[elem:N]`.
pub fn print_type(ty: &Type) -> String {
    let bt = ty.base_type.unwrap_or(BaseType::BASE_TYPE_NONE);
    match bt {
        BaseType::BASE_TYPE_VECTOR | BaseType::BASE_TYPE_VECTOR64 => {
            format!("[{}]", print_element(ty))
        }
        BaseType::BASE_TYPE_ARRAY => {
            let len = ty.fixed_length.unwrap_or(0);
            format!("[{}:{}]", print_element(ty), len)
        }
        BaseType::BASE_TYPE_TABLE | BaseType::BASE_TYPE_STRUCT | BaseType::BASE_TYPE_UNION => ty
            .unresolved_name
            .clone()
            .unwrap_or_else(|| "void".to_string()),
        scalar => {
            // A user-defined enum-typed field carries a scalar base_type but
            // also an `unresolved_name`; prefer the name when present.
            if let Some(name) = &ty.unresolved_name {
                name.clone()
            } else {
                base_type_name(scalar).to_string()
            }
        }
    }
}

/// Render the element type inside a vector/array `[...]`.
fn print_element(ty: &Type) -> String {
    if let Some(name) = &ty.unresolved_name {
        return name.clone();
    }
    let elem = ty.element_type.unwrap_or(BaseType::BASE_TYPE_NONE);
    base_type_name(elem).to_string()
}

/// Render leading documentation lines as `///` comments at the given indent.
fn print_doc(out: &mut String, doc: &Option<Documentation>, indent: &str) {
    if let Some(doc) = doc {
        for line in &doc.lines {
            out.push_str(indent);
            out.push_str("///");
            if !line.is_empty() {
                out.push(' ');
                out.push_str(line);
            }
            out.push('\n');
        }
    }
}

/// Render an attribute metadata block `(a, b: 2)` from parsed attributes.
///
/// The parser keeps system attributes (`id`, `deprecated`, `required`, ...)
/// inside `attributes.entries`, so printing them verbatim reproduces the
/// source faithfully without double-rendering.
fn print_attributes(attrs: &Option<Attributes>) -> String {
    let Some(attrs) = attrs else {
        return String::new();
    };
    if attrs.entries.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = attrs
        .entries
        .iter()
        .map(|kv| {
            let key = kv.key.as_deref().unwrap_or("");
            match &kv.value {
                Some(v) => format!("{key}: {v}"),
                None => key.to_string(),
            }
        })
        .collect();
    format!(" ({})", parts.join(", "))
}

/// Render a field default value clause, e.g. ` = 0` / ` = "x"`.
fn print_default(field: &Field) -> String {
    if let Some(i) = field.default_integer {
        return format!(" = {i}");
    }
    if let Some(r) = field.default_real {
        return format!(" = {r}");
    }
    if let Some(s) = &field.default_string {
        // Identifiers (enum value defaults) print bare; strings keep quotes.
        if s.is_empty() {
            return " = []".to_string();
        }
        return format!(" = {s}");
    }
    String::new()
}

/// Render one table/struct field line.
fn print_field(out: &mut String, field: &Field, indent: &str) {
    print_doc(out, &field.documentation, indent);
    out.push_str(indent);
    out.push_str(field.name.as_deref().unwrap_or("_"));
    out.push_str(": ");
    if let Some(ty) = &field.type_ {
        out.push_str(&print_type(ty));
    } else {
        out.push_str("void");
    }
    out.push_str(&print_default(field));
    out.push_str(&print_attributes(&field.attributes));
    out.push_str(";\n");
}

/// Render a `table`/`struct` declaration.
pub fn print_object(obj: &Object) -> String {
    let mut out = String::new();
    print_doc(&mut out, &obj.documentation, "");
    out.push_str(if obj.is_struct { "struct " } else { "table " });
    out.push_str(obj.name.as_deref().unwrap_or("_"));
    out.push_str(&print_attributes(&obj.attributes));
    out.push_str(" {\n");
    for field in &obj.fields {
        print_field(&mut out, field, "  ");
    }
    out.push_str("}\n");
    out
}

/// Render an `enum` or `union` declaration.
pub fn print_enum(e: &Enum) -> String {
    let mut out = String::new();
    print_doc(&mut out, &e.documentation, "");
    if e.is_union {
        out.push_str("union ");
        out.push_str(e.name.as_deref().unwrap_or("_"));
        out.push_str(&print_attributes(&e.attributes));
        out.push_str(" {\n");
        print_union_members(&mut out, &e.values);
    } else {
        out.push_str("enum ");
        out.push_str(e.name.as_deref().unwrap_or("_"));
        out.push_str(" : ");
        let underlying = e
            .underlying_type
            .as_ref()
            .and_then(|t| t.base_type)
            .map(base_type_name)
            .unwrap_or("int");
        out.push_str(underlying);
        out.push_str(&print_attributes(&e.attributes));
        out.push_str(" {\n");
        print_enum_values(&mut out, &e.values);
    }
    out.push_str("}\n");
    out
}

/// Render enum value lines (comma-separated, with explicit `= N`).
fn print_enum_values(out: &mut String, values: &[EnumVal]) {
    let len = values.len();
    for (i, val) in values.iter().enumerate() {
        print_doc(out, &val.documentation, "  ");
        out.push_str("  ");
        out.push_str(val.name.as_deref().unwrap_or("_"));
        if let Some(v) = val.value {
            out.push_str(&format!(" = {v}"));
        }
        if i + 1 < len {
            out.push(',');
        }
        out.push('\n');
    }
}

/// Render union member lines. The parser injects a synthetic `NONE` sentinel at
/// index 0 which is not part of the source — it is skipped here so re-parsing
/// re-creates exactly one `NONE`.
fn print_union_members(out: &mut String, values: &[EnumVal]) {
    let members: Vec<&EnumVal> = values
        .iter()
        .filter(|v| v.name.as_deref() != Some("NONE"))
        .collect();
    let len = members.len();
    for (i, val) in members.iter().enumerate() {
        print_doc(out, &val.documentation, "  ");
        out.push_str("  ");
        // The referenced type name is `union_type.unresolved_name`; the member's
        // own `name` is the source-level alias. For a positional member the
        // parser sets `name == type_name`, so only `Type` is emitted; for a
        // named member (`alias: Type`) the two differ and the alias is preserved.
        let type_name = val.union_type.as_ref().and_then(|t| t.unresolved_name.clone());
        let alias = val.name.clone();
        match (alias, type_name) {
            (Some(alias), Some(type_name)) if alias != type_name => {
                out.push_str(&alias);
                out.push_str(": ");
                out.push_str(&type_name);
            }
            (_, Some(type_name)) => out.push_str(&type_name),
            (Some(alias), None) => out.push_str(&alias),
            (None, None) => out.push('_'),
        }
        if i + 1 < len {
            out.push(',');
        }
        out.push('\n');
    }
}

/// Render an `rpc_service` declaration.
pub fn print_service(svc: &Service) -> String {
    let mut out = String::new();
    print_doc(&mut out, &svc.documentation, "");
    out.push_str("rpc_service ");
    out.push_str(svc.name.as_deref().unwrap_or("_"));
    out.push_str(" {\n");
    for call in &svc.calls {
        print_rpc(&mut out, call);
    }
    out.push_str("}\n");
    out
}

/// Render one rpc method line.
fn print_rpc(out: &mut String, call: &RpcCall) {
    print_doc(out, &call.documentation, "  ");
    out.push_str("  ");
    out.push_str(call.name.as_deref().unwrap_or("_"));
    out.push('(');
    out.push_str(
        call.request
            .as_ref()
            .and_then(|o| o.name.as_deref())
            .unwrap_or("void"),
    );
    out.push_str("): ");
    out.push_str(
        call.response
            .as_ref()
            .and_then(|o| o.name.as_deref())
            .unwrap_or("void"),
    );
    out.push_str(&print_attributes(&call.attributes));
    out.push_str(";\n");
}

/// Render one declaration payload.
pub fn print_decl(payload: &DeclPayload) -> String {
    match payload {
        DeclPayload::Object(o) => print_object(o),
        DeclPayload::Enum(e) => print_enum(e),
        DeclPayload::Service(s) => print_service(s),
    }
}

/// Render the file-level metadata header (namespace, includes, file_id/ext)
/// that precedes declarations.
pub fn print_meta_header(meta: &FbsMeta) -> String {
    let mut out = String::new();
    for inc in &meta.includes {
        out.push_str(&format!("include \"{inc}\";\n"));
    }
    if !meta.includes.is_empty() {
        out.push('\n');
    }
    if let Some(ns) = &meta.namespace {
        out.push_str(&format!("namespace {ns};\n\n"));
    }
    for attr in &meta.declared_attributes {
        out.push_str(&format!("attribute \"{attr}\";\n"));
    }
    if !meta.declared_attributes.is_empty() {
        out.push('\n');
    }
    if let Some(id) = &meta.file_ident {
        out.push_str(&format!("file_identifier \"{id}\";\n"));
    }
    if let Some(ext) = &meta.file_ext {
        out.push_str(&format!("file_extension \"{ext}\";\n"));
    }
    out
}

/// Render the trailing `root_type` declaration, if any.
pub fn print_root_type(meta: &FbsMeta) -> String {
    match &meta.root_type {
        Some(rt) => format!("root_type {rt};\n"),
        None => String::new(),
    }
}
