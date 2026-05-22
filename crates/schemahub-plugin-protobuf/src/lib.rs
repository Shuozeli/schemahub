pub mod ast;
pub mod compat;
pub mod diff;
pub mod mutations;
pub mod operations;
pub mod parser;
pub mod printer;

use std::collections::HashMap;

use bytes::Bytes;
use schemahub_types::{
    Blob, CompatibilityRules, CompatibilityViolation, DeclDetail, DeclKind, DeclSummary,
    DescriptorError, DiffError, FormatPlugin, Import, Language, Mutation, MutationError,
    ParseError, PrintError, ReadError, SchemaChange, SchemaPath,
    errors::CodegenError,
};

use crate::ast::{
    DeclBlob, FileMetadataBlob, ImportDef, ParseEnvelope, ParsedDecl, KIND_ENUM, KIND_MESSAGE,
    KIND_METADATA, KIND_SERVICE, decode_decl_blob, decode_decl_blob_print, decode_decl_blob_read,
    decode_envelope, decode_envelope_mutation, decode_envelope_print, encode_decl_blob,
    encode_envelope, unwrap_enum, unwrap_enum_print, unwrap_enum_read, unwrap_message,
    unwrap_message_print, unwrap_message_read, unwrap_metadata, unwrap_metadata_print,
    unwrap_metadata_read, unwrap_service, unwrap_service_print, unwrap_service_read, wrap_enum,
    wrap_message, wrap_metadata, wrap_service,
};
use crate::compat::{check_enum_compat, check_message_compat, check_service_compat};
use crate::diff::diff_decl_blobs;
use crate::mutations::{apply_add_enum_value, apply_to_message};
use crate::operations::{ProtoOp, decode_operation};
use crate::parser::parse_proto;
use crate::printer::{print_enum, print_message, print_service};

// ── ProtobufPlugin ────────────────────────────────────────────────────────────

pub struct ProtobufPlugin;

impl FormatPlugin for ProtobufPlugin {
    fn format_id(&self) -> &'static str {
        "protobuf"
    }

    // ── parse ─────────────────────────────────────────────────────────────────

    fn parse(&self, source: &str) -> Result<Blob, ParseError> {
        let proto_file = parse_proto(source)?;

        let mut declarations = Vec::new();

        // Metadata blob
        let metadata = FileMetadataBlob {
            blob_version: 1,
            syntax: proto_file.syntax.clone(),
            package: proto_file.package.clone(),
            imports: proto_file
                .imports
                .iter()
                .map(|p| ImportDef {
                    path: p.clone(),
                    resolved_commit: String::new(),
                })
                .collect(),
        };
        let meta_decl_blob = wrap_metadata(&metadata);
        declarations.push(ParsedDecl {
            tree_key: "__metadata__".into(),
            blob_bytes: encode_decl_blob(&meta_decl_blob),
        });

        // Messages
        for msg in &proto_file.messages {
            let decl_blob = wrap_message(msg);
            declarations.push(ParsedDecl {
                tree_key: msg.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Enums
        for e in &proto_file.enums {
            let decl_blob = wrap_enum(e);
            declarations.push(ParsedDecl {
                tree_key: e.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        // Services
        for svc in &proto_file.services {
            let decl_blob = wrap_service(svc);
            declarations.push(ParsedDecl {
                tree_key: svc.name.clone(),
                blob_bytes: encode_decl_blob(&decl_blob),
            });
        }

        let envelope = ParseEnvelope {
            blob_version: 1,
            declarations,
        };

        Ok(Blob::from(encode_envelope(&envelope)))
    }

    // ── print ─────────────────────────────────────────────────────────────────

    fn print(&self, blob: &Blob) -> Result<String, PrintError> {
        let data = blob.as_bytes();

        // Try to decode as ParseEnvelope first
        if let Ok(envelope) = decode_envelope_print(data) {
            if envelope.blob_version > 0 || !envelope.declarations.is_empty() {
                return print_envelope(&envelope);
            }
        }

        // Fall back to single DeclBlob
        let decl_blob = decode_decl_blob_print(data)?;
        print_single_decl_blob(&decl_blob)
    }

    // ── diff ──────────────────────────────────────────────────────────────────

    fn diff(&self, old: &Blob, new: &Blob) -> Result<Vec<SchemaChange>, DiffError> {
        // Both blobs are individual DeclBlobs (called per-declaration by the core)
        let maybe_detail = diff_decl_blobs(old.as_bytes(), new.as_bytes())?;

        if let Some(detail) = maybe_detail {
            // Determine the declaration name from the new blob
            let name = decl_name_from_blob(new.as_bytes()).unwrap_or_else(|| "unknown".into());
            Ok(vec![SchemaChange::DeclarationModified { name, detail }])
        } else {
            Ok(vec![])
        }
    }

    // ── apply_mutation ────────────────────────────────────────────────────────

    fn apply_mutation(&self, blob: &Blob, mutation: &Mutation) -> Result<Blob, MutationError> {
        use crate::ast::{FieldDef, MessageBlob, EnumValueDef, RpcDef, ServiceBlob};
        let op = decode_operation(&mutation.operation)?;
        let decl_name = &mutation.declaration_name;

        // Decode existing ParseEnvelope, or start empty
        let mut envelope = if blob.as_bytes().is_empty() {
            ParseEnvelope { blob_version: 1, declarations: vec![] }
        } else {
            decode_envelope_mutation(blob.as_bytes())?
        };

        // Schema-level (declaration create/delete/rename) ops
        match &op {
            ProtoOp::AddMessage(o) => {
                let msg = MessageBlob {
                    blob_version: 1,
                    name: o.message_name.clone(),
                    doc_comment: o.doc_comment.clone(),
                    ..Default::default()
                };
                envelope.declarations.push(ParsedDecl {
                    tree_key: o.message_name.clone(),
                    blob_bytes: encode_decl_blob(&wrap_message(&msg)),
                });
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            ProtoOp::RemoveMessage(o) => {
                envelope.declarations.retain(|d| d.tree_key != o.message_name);
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            ProtoOp::RenameMessage(o) => {
                if let Some(decl) = envelope.declarations.iter_mut().find(|d| d.tree_key == o.old_name) {
                    decl.tree_key = o.new_name.clone();
                    if let Ok(mut db) = decode_decl_blob(&decl.blob_bytes) {
                        if let Ok(mut msg) = unwrap_message(&db) {
                            msg.name = o.new_name.clone();
                            db = wrap_message(&msg);
                            decl.blob_bytes = encode_decl_blob(&db);
                        }
                    }
                }
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            ProtoOp::AddEnum(o) => {
                use crate::ast::EnumBlob;
                let e = EnumBlob {
                    blob_version: 1,
                    name: o.enum_name.clone(),
                    doc_comment: o.doc_comment.clone(),
                    ..Default::default()
                };
                envelope.declarations.push(ParsedDecl {
                    tree_key: o.enum_name.clone(),
                    blob_bytes: encode_decl_blob(&wrap_enum(&e)),
                });
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            ProtoOp::RemoveEnum(o) => {
                envelope.declarations.retain(|d| d.tree_key != o.enum_name);
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            ProtoOp::AddService(o) => {
                let svc = ServiceBlob {
                    blob_version: 1,
                    name: o.service_name.clone(),
                    doc_comment: o.doc_comment.clone(),
                    ..Default::default()
                };
                envelope.declarations.push(ParsedDecl {
                    tree_key: o.service_name.clone(),
                    blob_bytes: encode_decl_blob(&wrap_service(&svc)),
                });
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            ProtoOp::RemoveService(o) => {
                envelope.declarations.retain(|d| d.tree_key != o.service_name);
                return Ok(Blob::from(encode_envelope(&envelope)));
            }
            _ => {}
        }

        // Declaration-level (field/value) ops: find the target declaration
        let decl_idx = envelope.declarations.iter()
            .position(|d| d.tree_key == *decl_name)
            .ok_or_else(|| MutationError::InvalidOperation(
                format!("declaration '{}' not found in schema", decl_name)
            ))?;

        let decl_blob = decode_decl_blob(&envelope.declarations[decl_idx].blob_bytes)?;

        let new_decl_blob = match decl_blob.kind {
            KIND_MESSAGE => {
                let mut msg = unwrap_message(&decl_blob)?;
                match &op {
                    ProtoOp::AddField(o) => {
                        msg.fields.push(FieldDef {
                            name: o.field_name.clone(),
                            field_type: o.field_type.clone(),
                            number: o.field_number,
                            repeated: o.repeated,
                            doc_comment: o.doc_comment.clone(),
                            ..Default::default()
                        });
                    }
                    ProtoOp::RemoveField(o) => {
                        msg.fields.retain(|f| f.name != o.field_name);
                    }
                    ProtoOp::RenameField(o) => {
                        if let Some(f) = msg.fields.iter_mut().find(|f| f.name == o.old_field_name) {
                            f.name = o.new_field_name.clone();
                        }
                    }
                    ProtoOp::ChangeFieldType(o) => {
                        if let Some(f) = msg.fields.iter_mut().find(|f| f.name == o.field_name) {
                            f.field_type = o.new_type.clone();
                        }
                    }
                    ProtoOp::ChangeFieldLabel(o) => {
                        if let Some(f) = msg.fields.iter_mut().find(|f| f.name == o.field_name) {
                            f.repeated = o.new_label == "repeated";
                        }
                    }
                    ProtoOp::ReorderFields(o) => {
                        let ordered: Vec<FieldDef> = o.field_order.iter()
                            .filter_map(|name| msg.fields.iter().find(|f| f.name == *name).cloned())
                            .collect();
                        msg.fields = ordered;
                    }
                    _ => return Err(MutationError::UnsupportedInV1),
                }
                wrap_message(&msg)
            }
            KIND_ENUM => {
                let mut e = unwrap_enum(&decl_blob)?;
                match &op {
                    ProtoOp::AddEnumValue(o) => {
                        e.values.push(EnumValueDef {
                            name: o.value_name.clone(),
                            number: o.number,
                            doc_comment: o.doc_comment.clone(),
                        });
                    }
                    ProtoOp::RemoveEnumValue(o) => {
                        let before = e.values.len();
                        e.values.retain(|v| v.name != o.value_name);
                        if e.values.len() == before {
                            return Err(MutationError::InvalidOperation(format!(
                                "enum value '{}' not found in enum '{}'",
                                o.value_name, e.name
                            )));
                        }
                    }
                    ProtoOp::RenameEnumValue(o) => {
                        let v = e.values.iter_mut()
                            .find(|v| v.name == o.old_value_name)
                            .ok_or_else(|| MutationError::InvalidOperation(format!(
                                "enum value '{}' not found in enum '{}'",
                                o.old_value_name, e.name
                            )))?;
                        v.name = o.new_value_name.clone();
                    }
                    _ => return Err(MutationError::UnsupportedInV1),
                }
                wrap_enum(&e)
            }
            KIND_SERVICE => {
                let mut svc = unwrap_service(&decl_blob)?;
                match &op {
                    ProtoOp::AddRpc(o) => {
                        if svc.rpcs.iter().any(|r| r.name == o.rpc_name) {
                            return Err(MutationError::InvalidOperation(format!(
                                "rpc '{}' already exists in service '{}'",
                                o.rpc_name, svc.name
                            )));
                        }
                        svc.rpcs.push(RpcDef {
                            name: o.rpc_name.clone(),
                            request_type: o.request_type.clone(),
                            response_type: o.response_type.clone(),
                            client_streaming: o.client_streaming,
                            server_streaming: o.server_streaming,
                            doc_comment: o.doc_comment.clone(),
                        });
                    }
                    ProtoOp::RemoveRpc(o) => {
                        let before = svc.rpcs.len();
                        svc.rpcs.retain(|r| r.name != o.rpc_name);
                        if svc.rpcs.len() == before {
                            return Err(MutationError::InvalidOperation(format!(
                                "rpc '{}' not found in service '{}'",
                                o.rpc_name, svc.name
                            )));
                        }
                    }
                    ProtoOp::RenameRpc(o) => {
                        if svc.rpcs.iter().any(|r| r.name == o.new_rpc_name) {
                            return Err(MutationError::InvalidOperation(format!(
                                "rpc '{}' already exists in service '{}'",
                                o.new_rpc_name, svc.name
                            )));
                        }
                        let rpc = svc.rpcs.iter_mut()
                            .find(|r| r.name == o.old_rpc_name)
                            .ok_or_else(|| MutationError::InvalidOperation(format!(
                                "rpc '{}' not found in service '{}'",
                                o.old_rpc_name, svc.name
                            )))?;
                        rpc.name = o.new_rpc_name.clone();
                    }
                    _ => return Err(MutationError::UnsupportedInV1),
                }
                wrap_service(&svc)
            }
            other => {
                return Err(MutationError::MalformedBlob(format!("unknown DeclBlob kind {other}")));
            }
        };

        envelope.declarations[decl_idx].blob_bytes = encode_decl_blob(&new_decl_blob);
        Ok(Blob::from(encode_envelope(&envelope)))
    }

    // ── apply_mutations ───────────────────────────────────────────────────────

    fn apply_mutations(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
        mutations: &[Mutation],
    ) -> Result<HashMap<SchemaPath, Blob>, MutationError> {
        let mut result: HashMap<SchemaPath, Blob> = HashMap::new();

        // Work on a mutable copy that merges blobs + result
        let mut working: HashMap<SchemaPath, Blob> = blobs.clone();

        for mutation in mutations {
            let current = working
                .get(&mutation.schema_path)
                .cloned()
                .unwrap_or_else(|| Blob::from(vec![]));
            let new_blob = self.apply_mutation(&current, mutation)?;
            working.insert(mutation.schema_path.clone(), new_blob.clone());
            result.insert(mutation.schema_path.clone(), new_blob);
        }

        Ok(result)
    }

    // ── check_compatibility ───────────────────────────────────────────────────

    fn check_compatibility(
        &self,
        old: &Blob,
        new: &Blob,
        rules: &CompatibilityRules,
    ) -> Result<(), Vec<CompatibilityViolation>> {
        let direction = rules.direction;

        let old_env = decode_envelope_mutation(old.as_bytes())
            .unwrap_or_else(|_| ParseEnvelope { blob_version: 1, declarations: vec![] });
        let new_env = decode_envelope_mutation(new.as_bytes())
            .unwrap_or_else(|_| ParseEnvelope { blob_version: 1, declarations: vec![] });

        let mut all_violations: Vec<CompatibilityViolation> = Vec::new();

        for new_decl in &new_env.declarations {
            if new_decl.tree_key == "__metadata__" {
                continue;
            }
            let old_bytes = old_env.declarations.iter()
                .find(|d| d.tree_key == new_decl.tree_key)
                .map(|d| d.blob_bytes.as_slice());

            let old_db = match old_bytes {
                Some(b) => match decode_decl_blob(b) {
                    Ok(db) => db,
                    Err(_) => continue,
                },
                None => continue, // new declaration — no compat constraint
            };
            let new_db = match decode_decl_blob(&new_decl.blob_bytes) {
                Ok(db) => db,
                Err(_) => continue,
            };

            let violations = match (old_db.kind, new_db.kind) {
                (KIND_MESSAGE, KIND_MESSAGE) => {
                    match (unwrap_message(&old_db), unwrap_message(&new_db)) {
                        (Ok(old_msg), Ok(new_msg)) => check_message_compat(&old_msg, &new_msg, direction),
                        _ => vec![],
                    }
                }
                (KIND_ENUM, KIND_ENUM) => {
                    match (unwrap_enum(&old_db), unwrap_enum(&new_db)) {
                        (Ok(old_e), Ok(new_e)) => check_enum_compat(&old_e, &new_e, direction),
                        _ => vec![],
                    }
                }
                (KIND_SERVICE, KIND_SERVICE) => {
                    match (unwrap_service(&old_db), unwrap_service(&new_db)) {
                        (Ok(old_svc), Ok(new_svc)) => check_service_compat(&old_svc, &new_svc, direction),
                        _ => vec![],
                    }
                }
                _ => vec![],
            };
            all_violations.extend(violations);
        }

        if all_violations.is_empty() {
            Ok(())
        } else {
            Err(all_violations)
        }
    }

    // ── list_declarations ─────────────────────────────────────────────────────

    fn list_declarations(&self, blob: &Blob) -> Result<Vec<DeclSummary>, ReadError> {
        let envelope = decode_envelope(blob.as_bytes())?;
        let mut summaries = Vec::new();

        for parsed_decl in &envelope.declarations {
            if parsed_decl.tree_key == "__metadata__" {
                continue;
            }
            let decl_blob = decode_decl_blob_read(&parsed_decl.blob_bytes)?;
            let summary = match decl_blob.kind {
                KIND_MESSAGE => {
                    let msg = unwrap_message_read(&decl_blob)?;
                    DeclSummary {
                        name: msg.name.clone(),
                        kind: DeclKind::Message,
                        doc_comment: first_line(&msg.doc_comment),
                    }
                }
                KIND_ENUM => {
                    let e = unwrap_enum_read(&decl_blob)?;
                    DeclSummary {
                        name: e.name.clone(),
                        kind: DeclKind::Enum,
                        doc_comment: first_line(&e.doc_comment),
                    }
                }
                KIND_SERVICE => {
                    let svc = unwrap_service_read(&decl_blob)?;
                    DeclSummary {
                        name: svc.name.clone(),
                        kind: DeclKind::Service,
                        doc_comment: first_line(&svc.doc_comment),
                    }
                }
                KIND_METADATA => continue,
                other => {
                    return Err(ReadError::MalformedBlob(format!(
                        "unknown DeclBlob kind {other}"
                    )));
                }
            };
            summaries.push(summary);
        }

        Ok(summaries)
    }

    // ── get_declaration ───────────────────────────────────────────────────────

    fn get_declaration(&self, blob: &Blob, name: &str) -> Result<DeclDetail, ReadError> {
        let data = blob.as_bytes();

        // Try envelope first
        if let Ok(envelope) = decode_envelope(data) {
            if !envelope.declarations.is_empty() {
                let found = envelope
                    .declarations
                    .iter()
                    .find(|d| d.tree_key == name)
                    .ok_or_else(|| ReadError::NotFound(name.to_owned()))?;
                let decl_blob = decode_decl_blob_read(&found.blob_bytes)?;
                let text = print_single_decl_blob_read(&decl_blob)?;
                return Ok(DeclDetail::new(text));
            }
        }

        // Single DeclBlob
        let decl_blob = decode_decl_blob_read(data)?;
        let text = print_single_decl_blob_read(&decl_blob)?;
        Ok(DeclDetail::new(text))
    }

    // ── imports ───────────────────────────────────────────────────────────────

    fn imports(&self, blob: &Blob) -> Result<Vec<Import>, ReadError> {
        let envelope = decode_envelope(blob.as_bytes())?;

        let meta_decl = envelope
            .declarations
            .iter()
            .find(|d| d.tree_key == "__metadata__")
            .ok_or_else(|| ReadError::NotFound("__metadata__".into()))?;

        let meta_blob = decode_decl_blob_read(&meta_decl.blob_bytes)?;
        let metadata = unwrap_metadata_read(&meta_blob)?;

        Ok(metadata
            .imports
            .into_iter()
            .map(|imp| Import {
                path: imp.path,
                resolved_commit: imp.resolved_commit,
                decl_name: String::new(), // proto imports are file-level
            })
            .collect())
    }

    // ── generate_descriptors ──────────────────────────────────────────────────

    fn generate_descriptors(
        &self,
        blobs: &HashMap<SchemaPath, Blob>,
    ) -> Result<Bytes, DescriptorError> {
        use prost::Message as _;
        use prost_types::{
            DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto,
            FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
            MethodDescriptorProto, ServiceDescriptorProto,
            field_descriptor_proto::Label,
        };

        let mut files: Vec<FileDescriptorProto> = Vec::new();

        for (schema_path, blob) in blobs {
            let envelope = decode_envelope_mutation(blob.as_bytes())
                .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;

            let mut fdp = FileDescriptorProto {
                name: Some(schema_path.schema_name.clone()),
                syntax: Some("proto3".to_owned()),
                ..Default::default()
            };

            for decl in &envelope.declarations {
                let db = decode_decl_blob(&decl.blob_bytes)
                    .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;

                match db.kind {
                    KIND_METADATA => {
                        let meta = unwrap_metadata(&db)
                            .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;
                        if !meta.syntax.is_empty() {
                            fdp.syntax = Some(meta.syntax);
                        }
                        if !meta.package.is_empty() {
                            fdp.package = Some(meta.package);
                        }
                        for imp in &meta.imports {
                            fdp.dependency.push(imp.path.clone());
                        }
                    }
                    KIND_MESSAGE => {
                        let msg = unwrap_message(&db)
                            .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;
                        let mut dp = DescriptorProto {
                            name: Some(msg.name.clone()),
                            ..Default::default()
                        };
                        for field in &msg.fields {
                            let (ftype, type_name) = proto_scalar_type(&field.field_type);
                            dp.field.push(FieldDescriptorProto {
                                name: Some(field.name.clone()),
                                number: Some(field.number as i32),
                                label: Some(if field.repeated {
                                    Label::Repeated as i32
                                } else {
                                    Label::Optional as i32
                                }),
                                r#type: Some(ftype as i32),
                                type_name,
                                ..Default::default()
                            });
                        }
                        fdp.message_type.push(dp);
                    }
                    KIND_ENUM => {
                        let en = unwrap_enum(&db)
                            .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;
                        let mut edp = EnumDescriptorProto {
                            name: Some(en.name.clone()),
                            ..Default::default()
                        };
                        for v in &en.values {
                            edp.value.push(EnumValueDescriptorProto {
                                name: Some(v.name.clone()),
                                number: Some(v.number),
                                options: None,
                            });
                        }
                        fdp.enum_type.push(edp);
                    }
                    KIND_SERVICE => {
                        let svc = unwrap_service(&db)
                            .map_err(|e| DescriptorError::MalformedBlob(e.to_string()))?;
                        let mut sdp = ServiceDescriptorProto {
                            name: Some(svc.name.clone()),
                            ..Default::default()
                        };
                        for rpc in &svc.rpcs {
                            sdp.method.push(MethodDescriptorProto {
                                name: Some(rpc.name.clone()),
                                input_type: Some(qualify(&rpc.request_type, fdp.package.as_deref())),
                                output_type: Some(qualify(&rpc.response_type, fdp.package.as_deref())),
                                client_streaming: Some(rpc.client_streaming),
                                server_streaming: Some(rpc.server_streaming),
                                ..Default::default()
                            });
                        }
                        fdp.service.push(sdp);
                    }
                    _ => {}
                }
            }

            files.push(fdp);
        }

        let fds = FileDescriptorSet { file: files };
        Ok(Bytes::from(fds.encode_to_vec()))
    }

    // ── generate_code ─────────────────────────────────────────────────────────

    fn generate_code(
        &self,
        _blobs: &HashMap<SchemaPath, Blob>,
        language: Language,
    ) -> Result<String, CodegenError> {
        Err(CodegenError::UnsupportedLanguage(language))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Map a Protobuf scalar type name to a (Type, Option<type_name>) pair.
/// Unknown names are treated as message/enum references.
fn proto_scalar_type(
    field_type: &str,
) -> (prost_types::field_descriptor_proto::Type, Option<String>) {
    use prost_types::field_descriptor_proto::Type;
    match field_type {
        "double"   => (Type::Double, None),
        "float"    => (Type::Float, None),
        "int32"    => (Type::Int32, None),
        "int64"    => (Type::Int64, None),
        "uint32"   => (Type::Uint32, None),
        "uint64"   => (Type::Uint64, None),
        "sint32"   => (Type::Sint32, None),
        "sint64"   => (Type::Sint64, None),
        "fixed32"  => (Type::Fixed32, None),
        "fixed64"  => (Type::Fixed64, None),
        "sfixed32" => (Type::Sfixed32, None),
        "sfixed64" => (Type::Sfixed64, None),
        "bool"     => (Type::Bool, None),
        "string"   => (Type::String, None),
        "bytes"    => (Type::Bytes, None),
        other => (Type::Message, Some(qualify(other, None))),
    }
}

/// Turn a type reference into a fully-qualified `.package.TypeName` form.
fn qualify(type_ref: &str, package: Option<&str>) -> String {
    if type_ref.starts_with('.') {
        return type_ref.to_owned();
    }
    // If the reference already has dots (e.g. "acme.User"), use as-is from root
    if type_ref.contains('.') {
        return format!(".{}", type_ref);
    }
    // Unqualified name: scope under current package if known
    match package {
        Some(pkg) if !pkg.is_empty() => format!(".{}.{}", pkg, type_ref),
        _ => format!(".{}", type_ref),
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_owned()
}

fn decl_name_from_blob(data: &[u8]) -> Option<String> {
    let decl_blob = decode_decl_blob(data).ok()?;
    match decl_blob.kind {
        KIND_MESSAGE => {
            let msg = unwrap_message(&decl_blob).ok()?;
            Some(msg.name)
        }
        KIND_ENUM => {
            let e = unwrap_enum(&decl_blob).ok()?;
            Some(e.name)
        }
        KIND_SERVICE => {
            let svc = unwrap_service(&decl_blob).ok()?;
            Some(svc.name)
        }
        _ => None,
    }
}

fn print_envelope(envelope: &ParseEnvelope) -> Result<String, PrintError> {
    let mut out = String::new();

    // Find metadata
    let mut syntax = "proto3".to_owned();
    let mut package = String::new();
    let mut imports: Vec<String> = Vec::new();

    if let Some(meta_decl) = envelope.declarations.iter().find(|d| d.tree_key == "__metadata__") {
        let meta_blob = decode_decl_blob_print(&meta_decl.blob_bytes)?;
        let meta = unwrap_metadata_print(&meta_blob)?;
        syntax = meta.syntax;
        package = meta.package;
        imports = meta.imports.into_iter().map(|i| i.path).collect();
    }

    out.push_str(&format!("syntax = \"{syntax}\";\n"));
    if !package.is_empty() {
        out.push_str(&format!("package {package};\n"));
    }
    for import in &imports {
        out.push_str(&format!("import \"{import}\";\n"));
    }
    if !imports.is_empty() || !package.is_empty() {
        out.push('\n');
    }

    for parsed_decl in &envelope.declarations {
        if parsed_decl.tree_key == "__metadata__" {
            continue;
        }
        let decl_blob = decode_decl_blob_print(&parsed_decl.blob_bytes)?;
        let fragment = print_single_decl_blob(&decl_blob)?;
        out.push_str(&fragment);
        out.push('\n');
    }

    Ok(out)
}

fn print_single_decl_blob(decl_blob: &DeclBlob) -> Result<String, PrintError> {
    match decl_blob.kind {
        KIND_MESSAGE => {
            let msg = unwrap_message_print(decl_blob)?;
            Ok(print_message(&msg))
        }
        KIND_ENUM => {
            let e = unwrap_enum_print(decl_blob)?;
            Ok(print_enum(&e))
        }
        KIND_SERVICE => {
            let svc = unwrap_service_print(decl_blob)?;
            Ok(print_service(&svc))
        }
        KIND_METADATA => Ok(String::new()),
        other => Err(PrintError::MalformedBlob(format!(
            "unknown DeclBlob kind {other}"
        ))),
    }
}

fn print_single_decl_blob_read(decl_blob: &DeclBlob) -> Result<String, ReadError> {
    match decl_blob.kind {
        KIND_MESSAGE => {
            let msg = unwrap_message_read(decl_blob)?;
            Ok(print_message(&msg))
        }
        KIND_ENUM => {
            let e = unwrap_enum_read(decl_blob)?;
            Ok(print_enum(&e))
        }
        KIND_SERVICE => {
            let svc = unwrap_service_read(decl_blob)?;
            Ok(print_service(&svc))
        }
        KIND_METADATA => Ok(String::new()),
        other => Err(ReadError::MalformedBlob(format!(
            "unknown DeclBlob kind {other}"
        ))),
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_proto;

    const SOURCE: &str = r#"
        syntax = "proto3";
        package payments;

        // A payment creation request
        message CreatePaymentRequest {
          string user_id = 1;
          int64 amount_cents = 2;
        }

        message CreatePaymentResponse {
          string payment_id = 1;
        }

        enum PaymentStatus {
          UNKNOWN = 0;
          PENDING = 1;
          COMPLETED = 2;
        }

        service PaymentService {
          rpc CreatePayment (CreatePaymentRequest) returns (CreatePaymentResponse);
        }
    "#;

    #[test]
    fn plugin_parse_returns_envelope() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let decls = plugin.list_declarations(&blob).unwrap();
        let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"CreatePaymentRequest"), "missing message: {names:?}");
        assert!(names.contains(&"PaymentStatus"), "missing enum: {names:?}");
        assert!(names.contains(&"PaymentService"), "missing service: {names:?}");
    }

    #[test]
    fn plugin_list_declarations_kinds() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let decls = plugin.list_declarations(&blob).unwrap();

        let msg = decls.iter().find(|d| d.name == "CreatePaymentRequest").unwrap();
        assert_eq!(msg.kind, DeclKind::Message);

        let e = decls.iter().find(|d| d.name == "PaymentStatus").unwrap();
        assert_eq!(e.kind, DeclKind::Enum);

        let svc = decls.iter().find(|d| d.name == "PaymentService").unwrap();
        assert_eq!(svc.kind, DeclKind::Service);
    }

    #[test]
    fn plugin_imports() {
        let source = r#"
            syntax = "proto3";
            import "google/protobuf/timestamp.proto";
            message Foo { string x = 1; }
        "#;
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(source).unwrap();
        let imports = plugin.imports(&blob).unwrap();
        assert_eq!(imports.len(), 1);
        assert_eq!(imports[0].path, "google/protobuf/timestamp.proto");
    }

    #[test]
    fn plugin_get_declaration() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();
        let detail = plugin.get_declaration(&blob, "CreatePaymentRequest").unwrap();
        let text = std::str::from_utf8(detail.as_bytes()).unwrap();
        assert!(text.contains("CreatePaymentRequest"), "got: {text}");
        assert!(text.contains("user_id"));
    }

    #[test]
    fn plugin_print_roundtrip() {
        let plugin = ProtobufPlugin;
        let blob1 = plugin.parse(SOURCE).unwrap();
        let text = plugin.print(&blob1).unwrap();
        let blob2 = plugin.parse(&text).unwrap();

        let decls1 = plugin.list_declarations(&blob1).unwrap();
        let decls2 = plugin.list_declarations(&blob2).unwrap();

        let names1: Vec<&str> = decls1.iter().map(|d| d.name.as_str()).collect();
        let names2: Vec<&str> = decls2.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names1, names2, "declaration names differ after roundtrip");
    }

    #[test]
    fn parse_print_parse_roundtrip() {
        let plugin = ProtobufPlugin;
        let source = r#"
            syntax = "proto3";
            package test;
            message Foo {
              string name = 1;
              int32 count = 2;
            }
        "#;
        let blob1 = plugin.parse(source).unwrap();
        let printed = plugin.print(&blob1).unwrap();
        let blob2 = plugin.parse(&printed).unwrap();

        let parsed1 = parse_proto(source).unwrap();
        let parsed2 = parse_proto(&printed).unwrap();

        // field names and numbers should match
        assert_eq!(parsed1.messages[0].fields.len(), parsed2.messages[0].fields.len());
        for (f1, f2) in parsed1.messages[0].fields.iter().zip(parsed2.messages[0].fields.iter()) {
            assert_eq!(f1.name, f2.name);
            assert_eq!(f1.number, f2.number);
            assert_eq!(f1.field_type, f2.field_type);
        }

        // blobs should also match declaration names
        let decls1 = plugin.list_declarations(&blob1).unwrap();
        let decls2 = plugin.list_declarations(&blob2).unwrap();
        assert_eq!(decls1.len(), decls2.len());
    }

    #[test]
    fn plugin_diff_identical() {
        let plugin = ProtobufPlugin;
        let blob = plugin.parse(SOURCE).unwrap();

        // Get a single declaration blob to diff
        let envelope = crate::ast::decode_envelope(blob.as_bytes()).unwrap();
        let decl = envelope.declarations.iter().find(|d| d.tree_key == "CreatePaymentRequest").unwrap();
        let decl_blob = Blob::from(decl.blob_bytes.clone());
        let changes = plugin.diff(&decl_blob, &decl_blob).unwrap();
        assert!(changes.is_empty(), "identical blobs should produce no changes");
    }

    #[test]
    fn plugin_diff_detects_change() {
        let plugin = ProtobufPlugin;

        let source1 = r#"syntax = "proto3"; message Foo { string a = 1; }"#;
        let source2 = r#"syntax = "proto3"; message Foo { string a = 1; int32 b = 2; }"#;

        let blob1 = plugin.parse(source1).unwrap();
        let blob2 = plugin.parse(source2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = crate::ast::decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Foo");
        let new_decl = get_decl_blob(&blob2, "Foo");
        let changes = plugin.diff(&old_decl, &new_decl).unwrap();
        assert!(!changes.is_empty(), "should detect field addition");
    }

    #[test]
    fn plugin_check_compat_full_add_field_ok() {
        use schemahub_types::{CompatibilityDirection, CompatibilityRules};

        let plugin = ProtobufPlugin;
        let source1 = r#"syntax = "proto3"; message Foo { string a = 1; }"#;
        let source2 = r#"syntax = "proto3"; message Foo { string a = 1; int32 b = 2; }"#;

        let blob1 = plugin.parse(source1).unwrap();
        let blob2 = plugin.parse(source2).unwrap();

        fn get_decl_blob(blob: &Blob, name: &str) -> Blob {
            let env = crate::ast::decode_envelope(blob.as_bytes()).unwrap();
            let decl = env.declarations.iter().find(|d| d.tree_key == name).unwrap();
            Blob::from(decl.blob_bytes.clone())
        }

        let old_decl = get_decl_blob(&blob1, "Foo");
        let new_decl = get_decl_blob(&blob2, "Foo");

        let rules = CompatibilityRules { direction: CompatibilityDirection::Full };
        let result = plugin.check_compatibility(&old_decl, &new_decl, &rules);
        assert!(result.is_ok(), "adding a field should be FULL compatible: {result:?}");
    }

    #[test]
    fn generate_descriptors_produces_valid_file_descriptor_set() {
        use prost::Message as _;
        use prost_types::FileDescriptorSet;

        let plugin = ProtobufPlugin;
        let source = r#"
            syntax = "proto3";
            package payments;
            message CreatePaymentRequest {
                string user_id = 1;
                int64 amount_cents = 2;
            }
            enum Status { UNKNOWN = 0; ACTIVE = 1; }
        "#;

        let blob = plugin.parse(source).unwrap();
        let schema_path = SchemaPath::new("acme", "billing", "payment.proto");
        let blobs = HashMap::from([(schema_path, blob)]);

        let bytes = plugin.generate_descriptors(&blobs)
            .expect("generate_descriptors should succeed");
        assert!(!bytes.is_empty(), "descriptor bytes should not be empty");

        let fds = FileDescriptorSet::decode(bytes.as_ref())
            .expect("output must be a valid FileDescriptorSet");
        assert_eq!(fds.file.len(), 1);

        let fdp = &fds.file[0];
        assert_eq!(fdp.name.as_deref(), Some("payment.proto"));
        assert_eq!(fdp.package.as_deref(), Some("payments"));
        assert_eq!(fdp.syntax.as_deref(), Some("proto3"));

        // Message
        assert_eq!(fdp.message_type.len(), 1);
        let msg = &fdp.message_type[0];
        assert_eq!(msg.name.as_deref(), Some("CreatePaymentRequest"));
        assert_eq!(msg.field.len(), 2);
        let f0 = &msg.field[0];
        assert_eq!(f0.name.as_deref(), Some("user_id"));
        assert_eq!(f0.number, Some(1));

        // Enum
        assert_eq!(fdp.enum_type.len(), 1);
        let en = &fdp.enum_type[0];
        assert_eq!(en.name.as_deref(), Some("Status"));
        assert_eq!(en.value.len(), 2);
    }

    // ── New op tests ──────────────────────────────────────────────────────────

    fn make_mutation(decl_name: &str, tag: u32, payload: Vec<u8>) -> Mutation {
        use crate::operations::ProtoOperationEnvelope;
        Mutation {
            format_id: "protobuf".into(),
            schema_path: SchemaPath::new("p", "r", "test.proto"),
            declaration_name: decl_name.to_string(),
            operation: crate::operations::ProtoOperationEnvelope::encode_op(tag, payload).into(),
        }
    }

    #[test]
    fn apply_mutation_remove_enum() {
        use crate::operations::{op_tag, OpRemoveEnum};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; message Foo { string x = 1; } enum Status { UNKNOWN = 0; }"#;
        let blob = plugin.parse(source).unwrap();

        let decls_before = plugin.list_declarations(&blob).unwrap();
        assert!(decls_before.iter().any(|d| d.name == "Status"), "Status should exist before remove");

        let op = OpRemoveEnum { enum_name: "Status".into() };
        let mutation = make_mutation("", op_tag::REMOVE_ENUM, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let decls_after = plugin.list_declarations(&new_blob).unwrap();
        assert!(!decls_after.iter().any(|d| d.name == "Status"), "Status should be gone after remove");
        assert!(decls_after.iter().any(|d| d.name == "Foo"), "Foo should still exist");
    }

    #[test]
    fn apply_mutation_add_service() {
        use crate::operations::{op_tag, OpAddService};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; message Req { string x = 1; }"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpAddService { service_name: "FooService".into(), doc_comment: "A service".into() };
        let mutation = make_mutation("", op_tag::ADD_SERVICE, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let decls = plugin.list_declarations(&new_blob).unwrap();
        let svc = decls.iter().find(|d| d.name == "FooService").expect("FooService should exist");
        assert_eq!(svc.kind, DeclKind::Service);
    }

    #[test]
    fn apply_mutation_remove_service() {
        use crate::operations::{op_tag, OpRemoveService};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; service FooService { } message Req { string x = 1; }"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRemoveService { service_name: "FooService".into() };
        let mutation = make_mutation("", op_tag::REMOVE_SERVICE, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let decls = plugin.list_declarations(&new_blob).unwrap();
        assert!(!decls.iter().any(|d| d.name == "FooService"), "FooService should be removed");
        assert!(decls.iter().any(|d| d.name == "Req"), "Req should remain");
    }

    #[test]
    fn apply_mutation_add_rpc() {
        use crate::operations::{op_tag, OpAddRpc};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; service PaySvc { } message Req {} message Resp {}"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpAddRpc {
            rpc_name: "Process".into(),
            request_type: "Req".into(),
            response_type: "Resp".into(),
            client_streaming: false,
            server_streaming: false,
            doc_comment: String::new(),
        };
        let mutation = make_mutation("PaySvc", op_tag::ADD_RPC, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let text = plugin.print(&new_blob).unwrap();
        assert!(text.contains("rpc Process(Req) returns (Resp);"), "rpc should appear in printed output: {text}");
    }

    #[test]
    fn apply_mutation_add_rpc_duplicate_rejected() {
        use crate::operations::{op_tag, OpAddRpc};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; service PaySvc { rpc Process (Req) returns (Resp); } message Req {} message Resp {}"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpAddRpc {
            rpc_name: "Process".into(),
            request_type: "Req".into(),
            response_type: "Resp".into(),
            client_streaming: false,
            server_streaming: false,
            doc_comment: String::new(),
        };
        let mutation = make_mutation("PaySvc", op_tag::ADD_RPC, op.encode_to_vec());
        let result = plugin.apply_mutation(&blob, &mutation);
        assert!(result.is_err(), "adding duplicate rpc should be rejected");
    }

    #[test]
    fn apply_mutation_remove_rpc() {
        use crate::operations::{op_tag, OpRemoveRpc};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3";
            service PaySvc {
              rpc Process (Req) returns (Resp);
              rpc Cancel (Req) returns (Resp);
            }
            message Req {} message Resp {}"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRemoveRpc { rpc_name: "Process".into() };
        let mutation = make_mutation("PaySvc", op_tag::REMOVE_RPC, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let text = plugin.print(&new_blob).unwrap();
        assert!(!text.contains("rpc Process"), "Process rpc should be removed: {text}");
        assert!(text.contains("rpc Cancel"), "Cancel rpc should remain: {text}");
    }

    #[test]
    fn apply_mutation_remove_rpc_not_found_rejected() {
        use crate::operations::{op_tag, OpRemoveRpc};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; service PaySvc { } message Req {} message Resp {}"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRemoveRpc { rpc_name: "Nonexistent".into() };
        let mutation = make_mutation("PaySvc", op_tag::REMOVE_RPC, op.encode_to_vec());
        let result = plugin.apply_mutation(&blob, &mutation);
        assert!(result.is_err(), "removing nonexistent rpc should be rejected");
    }

    #[test]
    fn apply_mutation_rename_rpc() {
        use crate::operations::{op_tag, OpRenameRpc};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3";
            service PaySvc { rpc Process (Req) returns (Resp); }
            message Req {} message Resp {}"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRenameRpc { old_rpc_name: "Process".into(), new_rpc_name: "Execute".into() };
        let mutation = make_mutation("PaySvc", op_tag::RENAME_RPC, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let text = plugin.print(&new_blob).unwrap();
        assert!(!text.contains("rpc Process"), "old name should be gone: {text}");
        assert!(text.contains("rpc Execute"), "new name should appear: {text}");
    }

    #[test]
    fn apply_mutation_rename_rpc_conflict_rejected() {
        use crate::operations::{op_tag, OpRenameRpc};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3";
            service PaySvc {
              rpc Process (Req) returns (Resp);
              rpc Execute (Req) returns (Resp);
            }
            message Req {} message Resp {}"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRenameRpc { old_rpc_name: "Process".into(), new_rpc_name: "Execute".into() };
        let mutation = make_mutation("PaySvc", op_tag::RENAME_RPC, op.encode_to_vec());
        let result = plugin.apply_mutation(&blob, &mutation);
        assert!(result.is_err(), "renaming to an existing rpc name should be rejected");
    }

    #[test]
    fn apply_mutation_remove_enum_value() {
        use crate::operations::{op_tag, OpRemoveEnumValue};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3";
            enum Status { UNKNOWN = 0; ACTIVE = 1; DISABLED = 2; }"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRemoveEnumValue { enum_name: "Status".into(), value_name: "DISABLED".into() };
        let mutation = make_mutation("Status", op_tag::REMOVE_ENUM_VALUE, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let text = plugin.print(&new_blob).unwrap();
        assert!(!text.contains("DISABLED"), "DISABLED should be removed: {text}");
        assert!(text.contains("ACTIVE"), "ACTIVE should remain: {text}");
    }

    #[test]
    fn apply_mutation_remove_enum_value_not_found_rejected() {
        use crate::operations::{op_tag, OpRemoveEnumValue};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3"; enum Status { UNKNOWN = 0; }"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRemoveEnumValue { enum_name: "Status".into(), value_name: "NONEXISTENT".into() };
        let mutation = make_mutation("Status", op_tag::REMOVE_ENUM_VALUE, op.encode_to_vec());
        let result = plugin.apply_mutation(&blob, &mutation);
        assert!(result.is_err(), "removing nonexistent enum value should be rejected");
    }

    #[test]
    fn apply_mutation_rename_enum_value() {
        use crate::operations::{op_tag, OpRenameEnumValue};
        use prost::Message as _;

        let plugin = ProtobufPlugin;
        let source = r#"syntax = "proto3";
            enum Status { UNKNOWN = 0; ACTIVE = 1; }"#;
        let blob = plugin.parse(source).unwrap();

        let op = OpRenameEnumValue {
            enum_name: "Status".into(),
            old_value_name: "ACTIVE".into(),
            new_value_name: "ENABLED".into(),
        };
        let mutation = make_mutation("Status", op_tag::RENAME_ENUM_VALUE, op.encode_to_vec());
        let new_blob = plugin.apply_mutation(&blob, &mutation).unwrap();

        let text = plugin.print(&new_blob).unwrap();
        assert!(!text.contains("ACTIVE"), "old name should be gone: {text}");
        assert!(text.contains("ENABLED"), "new name should appear: {text}");
    }

    // ── BFS / multi-blob descriptor test ─────────────────────────────────────

    #[test]
    fn generate_descriptors_multi_blob_includes_imports() {
        use prost::Message as _;
        use prost_types::FileDescriptorSet;

        let plugin = ProtobufPlugin;

        // common.proto — a shared schema imported by payment.proto
        let common_src = r#"
            syntax = "proto3";
            package common;
            message Money { int64 amount_cents = 1; string currency = 2; }
        "#;
        let common_blob = plugin.parse(common_src).unwrap();
        let common_path = SchemaPath::new("acme", "billing", "common.proto");

        // payment.proto — imports common.proto
        let payment_src = r#"
            syntax = "proto3";
            package payments;
            import "common.proto";
            message CreatePaymentRequest { string user_id = 1; }
        "#;
        let payment_blob = plugin.parse(payment_src).unwrap();
        let payment_path = SchemaPath::new("acme", "billing", "payment.proto");

        // Simulate BFS result: both blobs present
        let blobs = HashMap::from([
            (payment_path, payment_blob),
            (common_path, common_blob),
        ]);

        let bytes = plugin.generate_descriptors(&blobs)
            .expect("generate_descriptors should succeed with multiple blobs");

        let fds = FileDescriptorSet::decode(bytes.as_ref())
            .expect("output must be a valid FileDescriptorSet");

        // Both files must appear in the descriptor set
        assert_eq!(fds.file.len(), 2, "expected 2 files in descriptor set: {fds:?}");
        let names: Vec<_> = fds.file.iter().filter_map(|f| f.name.as_deref()).collect();
        assert!(names.contains(&"payment.proto"), "payment.proto missing: {names:?}");
        assert!(names.contains(&"common.proto"), "common.proto missing: {names:?}");
    }

    #[test]
    fn imports_returns_declared_import_paths() {
        let plugin = ProtobufPlugin;
        let src = r#"
            syntax = "proto3";
            import "common.proto";
            import "google/protobuf/timestamp.proto";
            message Foo { string id = 1; }
        "#;
        let blob = plugin.parse(src).unwrap();
        let imports = plugin.imports(&blob).expect("imports() should succeed");

        let paths: Vec<_> = imports.iter().map(|i| i.path.as_str()).collect();
        assert!(paths.contains(&"common.proto"), "common.proto missing: {paths:?}");
        assert!(
            paths.contains(&"google/protobuf/timestamp.proto"),
            "timestamp import missing: {paths:?}"
        );
    }
}
