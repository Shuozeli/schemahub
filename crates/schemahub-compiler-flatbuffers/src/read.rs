//! Read / exploration helpers: `summarize_decl`, `decl_detail`, `imports`,
//! `type_refs` (design.md §2, §9).

use flatc_rs_schema::{BaseType, Enum, Object, Service, Type};
use schemahub_types::{
    DeclBlob, DeclDetail, DeclKind, DeclSummary, Import, MetaBlob, ReadError, TypeRef,
};

use crate::blob::{decode_decl, decode_meta, DeclPayload};

/// Map a malformed-blob decode error onto a [`ReadError`].
fn read_decode_err(e: crate::blob::BlobError) -> ReadError {
    ReadError::MalformedBlob(e.to_string())
}

/// The first line of the leading doc comment, or empty. The parser keeps the
/// space after `///`, so it is trimmed for a clean summary.
fn first_doc_line(lines: Option<&flatc_rs_schema::Documentation>) -> String {
    lines
        .and_then(|d| d.lines.first())
        .map(|l| l.trim().to_string())
        .unwrap_or_default()
}

/// Summarize one declaration (name, kind, leading doc line).
pub fn summarize_decl(blob: &DeclBlob) -> Result<DeclSummary, ReadError> {
    let payload = decode_decl(blob).map_err(read_decode_err)?;
    let (name, kind, doc) = match &payload {
        DeclPayload::Object(o) => {
            let kind = if o.is_struct {
                DeclKind::Struct
            } else {
                DeclKind::Table
            };
            (
                o.name.clone().unwrap_or_default(),
                kind,
                first_doc_line(o.documentation.as_ref()),
            )
        }
        DeclPayload::Enum(e) => {
            let kind = if e.is_union {
                DeclKind::Union
            } else {
                DeclKind::FbsEnum
            };
            (
                e.name.clone().unwrap_or_default(),
                kind,
                first_doc_line(e.documentation.as_ref()),
            )
        }
        DeclPayload::Service(s) => (
            s.name.clone().unwrap_or_default(),
            DeclKind::Service,
            first_doc_line(s.documentation.as_ref()),
        ),
    };
    Ok(DeclSummary {
        name,
        kind,
        doc_comment: doc,
    })
}

/// Full detail for one declaration: the canonical printed `.fbs` snippet bytes.
pub fn decl_detail(blob: &DeclBlob) -> Result<DeclDetail, ReadError> {
    let payload = decode_decl(blob).map_err(read_decode_err)?;
    let rendered = crate::printer::print_decl(&payload);
    Ok(DeclDetail::new(rendered.into_bytes()))
}

/// Imports declared in the file metadata. FlatBuffers `include "x.fbs"` has no
/// pinned-commit or decl granularity at the source level, so those fields are
/// left empty for the VCS layer to populate from its dependency index.
pub fn imports(meta: &MetaBlob) -> Result<Vec<Import>, ReadError> {
    let meta = decode_meta(meta).map_err(read_decode_err)?;
    Ok(meta
        .includes
        .into_iter()
        .enumerate()
        .map(|(index, path)| Import {
            path,
            resolved_commit: meta.include_commits.get(index).cloned().unwrap_or_default(),
            decl_name: String::new(),
        })
        .collect())
}

/// The type names this declaration references (for FollowType / rename
/// propagation). Only user-defined references (`unresolved_name`) are returned;
/// builtin scalars are skipped.
pub fn type_refs(blob: &DeclBlob) -> Result<Vec<TypeRef>, ReadError> {
    let payload = decode_decl(blob).map_err(read_decode_err)?;
    let mut refs: Vec<TypeRef> = Vec::new();
    let mut push = |name: Option<String>| {
        if let Some(n) = name {
            if !refs.iter().any(|r| r.name == n) {
                refs.push(TypeRef::new(n));
            }
        }
    };

    match &payload {
        DeclPayload::Object(o) => collect_object_refs(o, &mut push),
        DeclPayload::Enum(e) => collect_enum_refs(e, &mut push),
        DeclPayload::Service(s) => collect_service_refs(s, &mut push),
    }
    Ok(refs)
}

/// Resolve the user-defined type of one direct table/struct field. Builtin
/// scalar/string fields return `None`.
pub fn field_type_ref(blob: &DeclBlob, field_name: &str) -> Result<Option<TypeRef>, ReadError> {
    let payload = decode_decl(blob).map_err(read_decode_err)?;
    let DeclPayload::Object(object) = payload else {
        return Err(ReadError::FieldNotFound(field_name.to_string()));
    };
    let field = object
        .fields
        .iter()
        .find(|field| field.name.as_deref() == Some(field_name))
        .ok_or_else(|| ReadError::FieldNotFound(field_name.to_string()))?;
    Ok(field
        .type_
        .as_ref()
        .and_then(type_ref_name)
        .map(TypeRef::new))
}

/// The user-defined type name a `Type` references, if any.
fn type_ref_name(ty: &Type) -> Option<String> {
    let bt = ty.base_type.unwrap_or(BaseType::BASE_TYPE_NONE);
    match bt {
        BaseType::BASE_TYPE_TABLE
        | BaseType::BASE_TYPE_STRUCT
        | BaseType::BASE_TYPE_UNION
        | BaseType::BASE_TYPE_VECTOR
        | BaseType::BASE_TYPE_VECTOR64
        | BaseType::BASE_TYPE_ARRAY => ty.unresolved_name.clone(),
        _ => ty.unresolved_name.clone(),
    }
}

fn collect_object_refs(o: &Object, push: &mut impl FnMut(Option<String>)) {
    for f in &o.fields {
        if let Some(ty) = &f.type_ {
            push(type_ref_name(ty));
        }
    }
}

fn collect_enum_refs(e: &Enum, push: &mut impl FnMut(Option<String>)) {
    if e.is_union {
        for v in &e.values {
            if v.name.as_deref() == Some("NONE") {
                continue;
            }
            let name = v
                .union_type
                .as_ref()
                .and_then(|t| t.unresolved_name.clone())
                .or_else(|| v.name.clone());
            push(name);
        }
    }
}

fn collect_service_refs(s: &Service, push: &mut impl FnMut(Option<String>)) {
    for call in &s.calls {
        push(call.request.as_ref().and_then(|o| o.name.clone()));
        push(call.response.as_ref().and_then(|o| o.name.clone()));
    }
}
