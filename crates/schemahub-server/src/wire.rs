//! Wire conversion: `schemahub-api` (gRPC/prost) types ↔ `schemahub-core` /
//! `schemahub-types` types (crate-structure.md §3.6 `wire.rs`).
//!
//! The server is the composition root and the only place that knows how to turn
//! a typed gRPC mutation oneof into the opaque [`Mutation`] op envelope each
//! compiler interprets. It does so via each compiler crate's public op helpers:
//! protobuf → `ProtoOp::encode()`, flatbuffers → `FbsOp::encode()`, openapi →
//! `OpenApiOp::encode()`.

use bytes::Bytes;
use tonic::Status;

use schemahub_api::schemahub_v1 as pb;
use schemahub_compiler_flatbuffers::FbsOp;
use schemahub_compiler_openapi::operations::OpenApiOp;
use schemahub_compiler_protobuf::{
    OpAddEnumValue, OpAddField, OpAddRpc, OpAddService, OpChangeCardinality, OpChangeFieldType,
    OpChangeRpcType, OpCreateEnum, OpCreateMessage, OpDeleteEnum, OpDeleteMessage,
    OpRemoveEnumValue, OpRemoveField, OpRemoveRpc, OpRemoveService, OpRenameEnumValue,
    OpRenameField, OpRenameMessage, OpRenameRpc, OpRenameService, OpUpdateImport, ProtoOp,
};
use schemahub_jj::RefSpec;
use schemahub_types::{DeclKind, DeclSummary, Language, Mutation, SchemaPath};

/// Convert only an explicitly supplied version ref. An absent/empty message is
/// left as `None` so callers can use the repository's configured default.
pub fn version_ref_to_optional_refspec(at: &Option<pb::VersionRef>) -> Option<RefSpec> {
    match at.as_ref().and_then(|v| v.r#ref.as_ref()) {
        Some(pb::version_ref::Ref::Branch(b)) => RefSpec::bookmark(b.clone()),
        Some(pb::version_ref::Ref::Tag(t)) => RefSpec::Tag(t.clone()),
        Some(pb::version_ref::Ref::Commit(c)) => RefSpec::commit(c.clone()),
        None => return None,
    }
    .into()
}

/// Map a proto [`pb::SchemaFormat`] to a compiler format id.
pub fn format_id_from_pb(fmt: pb::SchemaFormat) -> Option<&'static str> {
    match fmt {
        pb::SchemaFormat::Protobuf => Some("protobuf"),
        pb::SchemaFormat::Flatbuffers => Some("flatbuffers"),
        pb::SchemaFormat::Openapi => Some("openapi"),
        pb::SchemaFormat::Unspecified => None,
    }
}

/// Map a [`DeclKind`] to its proto enum.
pub fn decl_kind_to_pb(kind: DeclKind) -> pb::DeclKind {
    match kind {
        DeclKind::Message => pb::DeclKind::Message,
        DeclKind::Enum => pb::DeclKind::Enum,
        DeclKind::Service => pb::DeclKind::Service,
        DeclKind::Table => pb::DeclKind::Table,
        DeclKind::Struct => pb::DeclKind::Struct,
        DeclKind::FbsEnum => pb::DeclKind::FbsEnum,
        DeclKind::Union => pb::DeclKind::Union,
        DeclKind::PathItem => pb::DeclKind::PathItem,
        DeclKind::ComponentSchema => pb::DeclKind::ComponentSchema,
        DeclKind::ComponentParameter => pb::DeclKind::ComponentParameter,
        DeclKind::ComponentResponse => pb::DeclKind::ComponentResponse,
        DeclKind::ComponentRequestBody => pb::DeclKind::ComponentRequestBody,
        DeclKind::DocumentMetadata => pb::DeclKind::DocumentMetadata,
    }
}

/// Map a proto [`pb::Language`] to a compiler [`Language`].
pub fn language_from_pb(lang: pb::Language) -> Result<Language, Status> {
    match lang {
        pb::Language::Rust => Ok(Language::Rust),
        pb::Language::Go => Ok(Language::Go),
        pb::Language::Typescript => Ok(Language::TypeScript),
        pb::Language::Python => Ok(Language::Python),
        pb::Language::Java => Ok(Language::Java),
        pb::Language::Unspecified => Err(Status::invalid_argument("language unspecified")),
    }
}

/// Build a core [`DeclSummary`]'s proto form.
pub fn decl_summary_to_pb(s: &DeclSummary) -> pb::DeclSummary {
    pb::DeclSummary {
        name: s.name.clone(),
        kind: decl_kind_to_pb(s.kind) as i32,
        doc_comment: s.doc_comment.clone(),
    }
}

// ── Mutation oneof → typed op envelope ───────────────────────────────────────

/// Convert a gRPC `ProtobufMutation` into a [`Mutation`] carrying a
/// `ProtoOp`-encoded operation. `project`/`repo` come from the request envelope.
pub fn protobuf_mutation_to_core(
    project: &str,
    repo: &str,
    m: &pb::ProtobufMutation,
) -> Result<Mutation, Status> {
    use pb::protobuf_mutation::Operation as Op;
    let op = m
        .operation
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("protobuf mutation: operation oneof not set"))?;

    let proto_op = match op {
        Op::AddField(f) => ProtoOp::AddField(OpAddField {
            message_name: f.message_name.clone(),
            field_name: f.field_name.clone(),
            field_type: f.field_type.clone(),
            field_number: f.field_number,
            cardinality: if f.repeated {
                "repeated".to_string()
            } else {
                String::new()
            },
        }),
        Op::RemoveField(f) => ProtoOp::RemoveField(OpRemoveField {
            message_name: f.message_name.clone(),
            field_name: f.field_name.clone(),
        }),
        Op::RenameField(f) => ProtoOp::RenameField(OpRenameField {
            message_name: f.message_name.clone(),
            old_field_name: f.old_field_name.clone(),
            new_field_name: f.new_field_name.clone(),
        }),
        Op::ChangeFieldType(f) => ProtoOp::ChangeFieldType(OpChangeFieldType {
            message_name: f.message_name.clone(),
            field_name: f.field_name.clone(),
            new_type: f.new_type.clone(),
        }),
        Op::ChangeFieldLabel(f) => ProtoOp::ChangeCardinality(OpChangeCardinality {
            message_name: f.message_name.clone(),
            field_name: f.field_name.clone(),
            new_cardinality: f.new_label.clone(),
        }),
        Op::ReorderFields(f) => {
            ProtoOp::ReorderFields(schemahub_compiler_protobuf::OpReorderFields {
                message_name: f.message_name.clone(),
                field_order: f.field_order.clone(),
            })
        }
        Op::AddMessage(m) => ProtoOp::CreateMessage(OpCreateMessage {
            message_name: m.message_name.clone(),
        }),
        Op::RemoveMessage(m) => ProtoOp::DeleteMessage(OpDeleteMessage {
            message_name: m.message_name.clone(),
        }),
        Op::RenameMessage(m) => ProtoOp::RenameMessage(OpRenameMessage {
            old_name: m.old_name.clone(),
            new_name: m.new_name.clone(),
        }),
        Op::AddEnum(e) => ProtoOp::CreateEnum(OpCreateEnum {
            enum_name: e.enum_name.clone(),
        }),
        Op::RemoveEnum(e) => ProtoOp::DeleteEnum(OpDeleteEnum {
            enum_name: e.enum_name.clone(),
        }),
        Op::AddEnumValue(e) => ProtoOp::AddEnumValue(OpAddEnumValue {
            enum_name: e.enum_name.clone(),
            value_name: e.value_name.clone(),
            number: e.number,
        }),
        Op::RemoveEnumValue(e) => ProtoOp::RemoveEnumValue(OpRemoveEnumValue {
            enum_name: e.enum_name.clone(),
            value_name: e.value_name.clone(),
        }),
        Op::RenameEnumValue(e) => ProtoOp::RenameEnumValue(OpRenameEnumValue {
            enum_name: e.enum_name.clone(),
            old_value_name: e.old_value_name.clone(),
            new_value_name: e.new_value_name.clone(),
        }),
        Op::AddService(s) => ProtoOp::AddService(OpAddService {
            service_name: s.service_name.clone(),
        }),
        Op::RemoveService(s) => ProtoOp::RemoveService(OpRemoveService {
            service_name: s.service_name.clone(),
        }),
        Op::AddRpc(r) => ProtoOp::AddRpc(OpAddRpc {
            service_name: r.service_name.clone(),
            rpc_name: r.rpc_name.clone(),
            request_type: r.request_type.clone(),
            response_type: r.response_type.clone(),
            client_streaming: r.client_streaming,
            server_streaming: r.server_streaming,
        }),
        Op::RemoveRpc(r) => ProtoOp::RemoveRpc(OpRemoveRpc {
            service_name: r.service_name.clone(),
            rpc_name: r.rpc_name.clone(),
        }),
        Op::RenameRpc(r) => ProtoOp::RenameRpc(OpRenameRpc {
            service_name: r.service_name.clone(),
            old_rpc_name: r.old_rpc_name.clone(),
            new_rpc_name: r.new_rpc_name.clone(),
        }),
        Op::RenameService(service) => ProtoOp::RenameService(OpRenameService {
            old_name: service.old_name.clone(),
            new_name: service.new_name.clone(),
        }),
        Op::ChangeRpcType(rpc) => ProtoOp::ChangeRpcType(OpChangeRpcType {
            service_name: rpc.service_name.clone(),
            rpc_name: rpc.rpc_name.clone(),
            new_request_type: rpc.new_request_type.clone(),
            new_response_type: rpc.new_response_type.clone(),
        }),
        Op::UpdateImport(i) => ProtoOp::UpdateImport(OpUpdateImport {
            import_path: i.import_path.clone(),
            resolved_commit: validated_import_commit(
                "protobuf",
                &i.import_path,
                &i.to_commit,
                &i.to_tag,
                i.remove,
            )?,
            remove: i.remove,
        }),
    };

    Ok(Mutation {
        schema_path: SchemaPath::new(project, repo, m.schema_path.clone()),
        format_id: "protobuf".to_string(),
        operation: Bytes::from(proto_op.encode()),
    })
}

/// Convert a gRPC `FlatBuffersMutation` into a [`Mutation`].
pub fn fbs_mutation_to_core(
    project: &str,
    repo: &str,
    m: &pb::FlatBuffersMutation,
) -> Result<Mutation, Status> {
    use pb::flat_buffers_mutation::Operation as Op;
    let op = m
        .operation
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("flatbuffers mutation: operation oneof not set"))?;

    let opt = |s: &str| (!s.is_empty()).then(|| s.to_string());

    let fbs_op = match op {
        Op::AddField(f) => FbsOp::AddField {
            table: f.table_name.clone(),
            field_name: f.field_name.clone(),
            field_type: f.field_type.clone(),
            default_value: opt(&f.default_value),
            doc_comment: opt(&f.doc_comment),
        },
        Op::DeprecateField(f) => FbsOp::DeprecateField {
            table: f.table_name.clone(),
            field_name: f.field_name.clone(),
        },
        Op::RenameField(f) => FbsOp::RenameField {
            table: f.table_name.clone(),
            old_name: f.old_field_name.clone(),
            new_name: f.new_field_name.clone(),
        },
        Op::ChangeFieldType(field) => FbsOp::ChangeFieldType {
            table: field.table_name.clone(),
            field_name: field.field_name.clone(),
            new_type: field.new_type.clone(),
        },
        Op::AddTable(t) => FbsOp::CreateTable {
            name: t.table_name.clone(),
            doc_comment: opt(&t.doc_comment),
        },
        Op::RemoveTable(t) => FbsOp::DeleteTable {
            name: t.table_name.clone(),
        },
        Op::RenameTable(t) => FbsOp::RenameTable {
            old_name: t.old_name.clone(),
            new_name: t.new_name.clone(),
        },
        Op::AddEnum(e) => FbsOp::CreateEnum {
            name: e.enum_name.clone(),
            underlying_type: e.base_type.clone(),
            doc_comment: opt(&e.doc_comment),
        },
        Op::AddEnumValue(e) => FbsOp::AddEnumValue {
            enum_name: e.enum_name.clone(),
            value_name: e.value_name.clone(),
            value: e.value,
        },
        Op::RemoveEnum(e) => FbsOp::DeleteEnum {
            name: e.enum_name.clone(),
        },
        Op::RenameEnum(e) => FbsOp::RenameEnum {
            old_name: e.old_name.clone(),
            new_name: e.new_name.clone(),
        },
        Op::RemoveEnumValue(e) => FbsOp::RemoveEnumValue {
            enum_name: e.enum_name.clone(),
            value_name: e.value_name.clone(),
        },
        Op::RenameEnumValue(e) => FbsOp::RenameEnumValue {
            enum_name: e.enum_name.clone(),
            old_name: e.old_value_name.clone(),
            new_name: e.new_value_name.clone(),
        },
        Op::AddUnion(u) => FbsOp::CreateUnionWithMembers {
            name: u.union_name.clone(),
            member_types: u.member_types.clone(),
            doc_comment: opt(&u.doc_comment),
        },
        Op::AddUnionMember(member) => FbsOp::AddUnionMember {
            union_name: member.union_name.clone(),
            member_type: member.member_type.clone(),
        },
        Op::RemoveUnionMember(member) => FbsOp::RemoveUnionMember {
            union_name: member.union_name.clone(),
            member_type: member.member_type.clone(),
        },
        Op::RemoveUnion(union) => FbsOp::DeleteUnion {
            name: union.union_name.clone(),
        },
        Op::RenameUnion(union) => FbsOp::RenameUnion {
            old_name: union.old_name.clone(),
            new_name: union.new_name.clone(),
        },
        Op::UpdateImport(i) => FbsOp::UpdateImport {
            import_path: i.import_path.clone(),
            resolved_commit: validated_import_commit(
                "flatbuffers",
                &i.import_path,
                &i.to_commit,
                &i.to_tag,
                i.remove,
            )?,
            remove: i.remove,
        },
    };

    Ok(Mutation {
        schema_path: SchemaPath::new(project, repo, m.schema_path.clone()),
        format_id: "flatbuffers".to_string(),
        operation: fbs_op.encode(),
    })
}

/// Convert a gRPC `OpenApiMutation` into a [`Mutation`].
pub fn openapi_mutation_to_core(
    project: &str,
    repo: &str,
    m: &pb::OpenApiMutation,
) -> Result<Mutation, Status> {
    use pb::open_api_mutation::Operation as Op;
    let op = m
        .operation
        .as_ref()
        .ok_or_else(|| Status::invalid_argument("openapi mutation: operation oneof not set"))?;

    let oa_op = match op {
        Op::PushDocument(d) => OpenApiOp::PushDocument {
            source: d.source.clone(),
        },
        Op::AddPath(p) => OpenApiOp::AddPath {
            path_pattern: p.path_pattern.clone(),
            summary: p.summary.clone(),
            description: p.description.clone(),
        },
        Op::RemovePath(p) => OpenApiOp::RemovePath {
            path_pattern: p.path_pattern.clone(),
        },
        Op::AddOperation(o) => OpenApiOp::AddOperation {
            path_pattern: o.path_pattern.clone(),
            method: o.method.clone(),
            operation_id: o.operation_id.clone(),
            summary: o.summary.clone(),
            description: o.description.clone(),
        },
        Op::RemoveOperation(o) => OpenApiOp::RemoveOperation {
            path_pattern: o.path_pattern.clone(),
            method: o.method.clone(),
        },
        Op::AddComponentSchema(c) => OpenApiOp::AddComponentSchema {
            schema_name: c.schema_name.clone(),
            schema_type: c.schema_type.clone(),
            description: c.description.clone(),
        },
        Op::RemoveComponentSchema(c) => OpenApiOp::RemoveComponentSchema {
            schema_name: c.schema_name.clone(),
        },
    };

    Ok(Mutation {
        schema_path: SchemaPath::new(project, repo, m.schema_path.clone()),
        format_id: "openapi".to_string(),
        operation: Bytes::from(oa_op.encode()),
    })
}

/// Convert one `ApplyMutationRequest.operation` oneof into a [`Mutation`].
pub fn apply_mutation_op_to_core(
    project: &str,
    repo: &str,
    op: &pb::apply_mutation_request::Operation,
) -> Result<Mutation, Status> {
    match op {
        pb::apply_mutation_request::Operation::ProtobufOp(m) => {
            protobuf_mutation_to_core(project, repo, m)
        }
        pb::apply_mutation_request::Operation::FbsOp(m) => fbs_mutation_to_core(project, repo, m),
        pb::apply_mutation_request::Operation::OpenapiOp(m) => {
            openapi_mutation_to_core(project, repo, m)
        }
    }
}

/// Convert one `TransactionOp` into a [`Mutation`].
pub fn transaction_op_to_core(
    project: &str,
    repo: &str,
    op: &pb::transaction_op::Operation,
) -> Result<Mutation, Status> {
    match op {
        pb::transaction_op::Operation::ProtobufOp(m) => protobuf_mutation_to_core(project, repo, m),
        pb::transaction_op::Operation::FbsOp(m) => fbs_mutation_to_core(project, repo, m),
        pb::transaction_op::Operation::OpenapiOp(m) => openapi_mutation_to_core(project, repo, m),
    }
}

fn validated_import_commit(
    format_id: &str,
    import_path: &str,
    to_commit: &str,
    to_tag: &str,
    remove: bool,
) -> Result<String, Status> {
    if import_path.trim().is_empty() {
        return Err(Status::invalid_argument(format!(
            "{format_id} import_path must not be empty"
        )));
    }
    if !to_commit.is_empty() && !to_tag.is_empty() {
        return Err(Status::invalid_argument(format!(
            "{format_id} import update accepts at most one of to_commit and to_tag"
        )));
    }
    if remove && (!to_commit.is_empty() || !to_tag.is_empty()) {
        return Err(Status::invalid_argument(format!(
            "{format_id} import removal must not include a commit or tag pin"
        )));
    }
    if !to_tag.is_empty() {
        return Err(Status::invalid_argument(format!(
            "{format_id} import tag was not resolved by the schema service"
        )));
    }
    Ok(to_commit.to_string())
}

/// Detect the schema-file path inside a mutation oneof (so the request envelope
/// can validate it / use it as the commit target).
pub fn mutation_op_schema_path(op: &pb::apply_mutation_request::Operation) -> &str {
    match op {
        pb::apply_mutation_request::Operation::ProtobufOp(m) => &m.schema_path,
        pb::apply_mutation_request::Operation::FbsOp(m) => &m.schema_path,
        pb::apply_mutation_request::Operation::OpenapiOp(m) => &m.schema_path,
    }
}
