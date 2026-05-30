//! Apply granular mutations against the real `protoc-rs-schema` descriptors
//! (design.md §3.1 mutation validator).

use protoc_rs_schema::{
    DescriptorProto, EnumDescriptorProto, EnumValueDescriptorProto, FieldDescriptorProto,
    FieldLabel, FieldType, MethodDescriptorProto, OneofDescriptorProto, ReservedRange,
    ServiceDescriptorProto,
};
use schemahub_types::{DeclBlob, MetaBlob, Mutation, MutationEffect, MutationError, SchemaObjects};

use crate::blob::{decode_decl, decode_meta, encode_decl, encode_meta, DeclPayload};
use crate::operations::*;
use crate::typecompat::{scalar_field_type, type_change_allowed_typed};

/// Apply a single mutation, returning its effect.
pub fn apply_one(schema: &SchemaObjects, op: &Mutation) -> Result<MutationEffect, MutationError> {
    let parsed = ProtoOp::decode(&op.operation)?;
    let mut state = WorkingState::load(schema)?;
    state.apply(&parsed)?;
    Ok(state.into_effect(schema))
}

/// Apply an ordered batch; only the final state matters.
pub fn apply_many(
    schema: &SchemaObjects,
    ops: &[Mutation],
) -> Result<MutationEffect, MutationError> {
    let mut state = WorkingState::load(schema)?;
    for op in ops {
        let parsed = ProtoOp::decode(&op.operation)?;
        state.apply(&parsed)?;
    }
    Ok(state.into_effect(schema))
}

/// A mutable in-memory view of the schema's decls and meta during a mutation.
struct WorkingState {
    messages: std::collections::BTreeMap<String, DescriptorProto>,
    enums: std::collections::BTreeMap<String, EnumDescriptorProto>,
    services: std::collections::BTreeMap<String, ServiceDescriptorProto>,
    meta: crate::blob::MetaPayload,
    meta_dirty: bool,
    touched: std::collections::BTreeSet<String>,
    removed: std::collections::BTreeSet<String>,
}

impl WorkingState {
    fn load(schema: &SchemaObjects) -> Result<Self, MutationError> {
        let mut messages = std::collections::BTreeMap::new();
        let mut enums = std::collections::BTreeMap::new();
        let mut services = std::collections::BTreeMap::new();
        for (name, blob) in &schema.decls {
            match decode_decl(blob.as_bytes())
                .map_err(|e| MutationError::MalformedBlob(e.to_string()))?
            {
                DeclPayload::Message(m) => {
                    messages.insert(name.clone(), m);
                }
                DeclPayload::Enum(e) => {
                    enums.insert(name.clone(), e);
                }
                DeclPayload::Service(s) => {
                    services.insert(name.clone(), s);
                }
            }
        }
        let meta = decode_meta(schema.meta.as_bytes())
            .map_err(|e| MutationError::MalformedBlob(e.to_string()))?;
        Ok(Self {
            messages,
            enums,
            services,
            meta,
            meta_dirty: false,
            touched: Default::default(),
            removed: Default::default(),
        })
    }

    fn into_effect(self, original: &SchemaObjects) -> MutationEffect {
        let mut upserts = Vec::new();
        for name in &self.touched {
            if self.removed.contains(name) {
                continue;
            }
            if let Some(m) = self.messages.get(name) {
                upserts.push((name.clone(), DeclBlob::new(encode_decl(&DeclPayload::Message(m.clone())))));
            } else if let Some(e) = self.enums.get(name) {
                upserts.push((name.clone(), DeclBlob::new(encode_decl(&DeclPayload::Enum(e.clone())))));
            } else if let Some(s) = self.services.get(name) {
                upserts.push((name.clone(), DeclBlob::new(encode_decl(&DeclPayload::Service(s.clone())))));
            }
        }
        let removes: Vec<String> = self
            .removed
            .iter()
            .filter(|n| original.decls.contains_key(*n))
            .cloned()
            .collect();
        let meta = if self.meta_dirty {
            Some(MetaBlob::new(encode_meta(&self.meta)))
        } else {
            None
        };
        MutationEffect {
            meta,
            upserts,
            removes,
        }
    }

    fn touch(&mut self, name: &str) {
        self.touched.insert(name.to_string());
        self.removed.remove(name);
    }

    fn message_mut(&mut self, name: &str) -> Result<&mut DescriptorProto, MutationError> {
        self.messages
            .get_mut(name)
            .ok_or_else(|| MutationError::DeclarationNotFound(name.to_string()))
    }

    fn enum_mut(&mut self, name: &str) -> Result<&mut EnumDescriptorProto, MutationError> {
        self.enums
            .get_mut(name)
            .ok_or_else(|| MutationError::DeclarationNotFound(name.to_string()))
    }

    fn service_mut(&mut self, name: &str) -> Result<&mut ServiceDescriptorProto, MutationError> {
        self.services
            .get_mut(name)
            .ok_or_else(|| MutationError::DeclarationNotFound(name.to_string()))
    }

    fn apply(&mut self, op: &ProtoOp) -> Result<(), MutationError> {
        match op {
            ProtoOp::AddField(o) => self.add_field(o),
            ProtoOp::RemoveField(o) => self.remove_field(o),
            ProtoOp::RenameField(o) => self.rename_field(o),
            ProtoOp::ChangeFieldType(o) => self.change_field_type(o),
            ProtoOp::ChangeCardinality(o) => self.change_cardinality(o),
            ProtoOp::ReorderFields(o) => self.reorder_fields(o),
            ProtoOp::ChangeFieldNumber(_) => Err(MutationError::InvalidOperation(
                "ChangeFieldNumber is always breaking and is rejected".into(),
            )),
            ProtoOp::CreateMessage(o) => self.create_message(o),
            ProtoOp::RenameMessage(o) => self.rename_message(o),
            ProtoOp::DeleteMessage(o) => self.delete_message(o),
            ProtoOp::CreateEnum(o) => self.create_enum(o),
            ProtoOp::DeleteEnum(o) => self.delete_enum(o),
            ProtoOp::AddEnumValue(o) => self.add_enum_value(o),
            ProtoOp::RemoveEnumValue(o) => self.remove_enum_value(o),
            ProtoOp::RenameEnumValue(o) => self.rename_enum_value(o),
            ProtoOp::AddService(o) => self.add_service(o),
            ProtoOp::RemoveService(o) => self.remove_service(o),
            ProtoOp::RenameService(o) => self.rename_service(o),
            ProtoOp::AddRpc(o) => self.add_rpc(o),
            ProtoOp::RemoveRpc(o) => self.remove_rpc(o),
            ProtoOp::RenameRpc(o) => self.rename_rpc(o),
            ProtoOp::ChangeRpcType(o) => self.change_rpc_type(o),
            ProtoOp::UpdateImport(o) => self.update_import(o),
        }
    }

    // ── Field ops ───────────────────────────────────────────────────────────

    fn add_field(&mut self, o: &OpAddField) -> Result<(), MutationError> {
        let number = o.field_number as i32;
        let msg = self.message_mut(&o.message_name)?;
        if msg.field.iter().any(|f| f.number == Some(number)) {
            return Err(MutationError::FieldNumberConflict(o.field_number));
        }
        if is_number_reserved(msg, number) {
            return Err(MutationError::FieldNumberReserved(o.field_number));
        }
        let (label, proto3_optional) = cardinality_to_label(&o.cardinality)?;
        let (ty, type_name) = resolve_type(&o.field_type);
        let oneof_index = if proto3_optional {
            // Create a synthetic oneof `_<name>` for proto3 optional.
            let idx = msg.oneof_decl.len() as i32;
            msg.oneof_decl.push(OneofDescriptorProto {
                name: Some(format!("_{}", o.field_name)),
                options: None,
            });
            Some(idx)
        } else {
            None
        };
        msg.field.push(FieldDescriptorProto {
            name: Some(o.field_name.clone()),
            number: Some(number),
            label,
            r#type: ty,
            type_name,
            extendee: None,
            default_value: None,
            oneof_index,
            json_name: Some(protoc_rs_parser::to_camel_case(&o.field_name)),
            options: None,
            proto3_optional: if proto3_optional { Some(true) } else { None },
            source_span: None,
        });
        self.touch(&o.message_name);
        Ok(())
    }

    fn remove_field(&mut self, o: &OpRemoveField) -> Result<(), MutationError> {
        let msg = self.message_mut(&o.message_name)?;
        let idx = msg
            .field
            .iter()
            .position(|f| f.name.as_deref() == Some(o.field_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.message_name.clone(),
                field: o.field_name.clone(),
            })?;
        let removed = msg.field.remove(idx);
        // Auto-reserve the removed number and name (design.md §3.1).
        if let Some(num) = removed.number {
            msg.reserved_range.push(ReservedRange {
                start: Some(num),
                end: Some(num + 1), // end is exclusive
            });
        }
        if let Some(name) = removed.name {
            msg.reserved_name.push(name);
        }
        self.touch(&o.message_name);
        Ok(())
    }

    fn rename_field(&mut self, o: &OpRenameField) -> Result<(), MutationError> {
        let msg = self.message_mut(&o.message_name)?;
        let field = msg
            .field
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(o.old_field_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.message_name.clone(),
                field: o.old_field_name.clone(),
            })?;
        field.name = Some(o.new_field_name.clone());
        field.json_name = Some(protoc_rs_parser::to_camel_case(&o.new_field_name));
        self.touch(&o.message_name);
        Ok(())
    }

    fn change_field_type(&mut self, o: &OpChangeFieldType) -> Result<(), MutationError> {
        let msg = self.message_mut(&o.message_name)?;
        let field = msg
            .field
            .iter()
            .find(|f| f.name.as_deref() == Some(o.field_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.message_name.clone(),
                field: o.field_name.clone(),
            })?;
        let from = current_type_name(field);
        let from_ty = field.r#type;
        let (ty, type_name) = resolve_type(&o.new_type);
        type_change_allowed_typed(&from, from_ty, &o.new_type, ty)
            .map_err(MutationError::InvalidOperation)?;
        let field = msg
            .field
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(o.field_name.as_str()))
            .unwrap();
        field.r#type = ty;
        field.type_name = type_name;
        self.touch(&o.message_name);
        Ok(())
    }

    fn change_cardinality(&mut self, o: &OpChangeCardinality) -> Result<(), MutationError> {
        let (label, proto3_optional) = cardinality_to_label(&o.new_cardinality)?;
        let msg = self.message_mut(&o.message_name)?;
        let field = msg
            .field
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(o.field_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.message_name.clone(),
                field: o.field_name.clone(),
            })?;
        field.label = label;
        field.proto3_optional = if proto3_optional { Some(true) } else { None };
        self.touch(&o.message_name);
        Ok(())
    }

    fn reorder_fields(&mut self, o: &OpReorderFields) -> Result<(), MutationError> {
        let msg = self.message_mut(&o.message_name)?;
        let existing: std::collections::HashSet<&str> =
            msg.field.iter().filter_map(|f| f.name.as_deref()).collect();
        let wanted: std::collections::HashSet<&str> =
            o.field_order.iter().map(|s| s.as_str()).collect();
        if existing != wanted {
            return Err(MutationError::InvalidOperation(
                "ReorderFields: new order must list exactly the existing fields".into(),
            ));
        }
        let mut by_name: std::collections::HashMap<String, FieldDescriptorProto> = msg
            .field
            .drain(..)
            .map(|f| (f.name.clone().unwrap_or_default(), f))
            .collect();
        for name in &o.field_order {
            if let Some(f) = by_name.remove(name) {
                msg.field.push(f);
            }
        }
        self.touch(&o.message_name);
        Ok(())
    }

    // ── Message ops ───────────────────────────────────────────────────────────

    fn create_message(&mut self, o: &OpCreateMessage) -> Result<(), MutationError> {
        if self.messages.contains_key(&o.message_name) {
            return Err(MutationError::InvalidOperation(format!(
                "message '{}' already exists",
                o.message_name
            )));
        }
        self.messages.insert(
            o.message_name.clone(),
            DescriptorProto {
                name: Some(o.message_name.clone()),
                ..Default::default()
            },
        );
        self.touch(&o.message_name);
        Ok(())
    }

    fn rename_message(&mut self, o: &OpRenameMessage) -> Result<(), MutationError> {
        let mut m = self
            .messages
            .remove(&o.old_name)
            .ok_or_else(|| MutationError::DeclarationNotFound(o.old_name.clone()))?;
        m.name = Some(o.new_name.clone());
        // A self-reference inside the renamed message must update too.
        rename_refs_in_message(&mut m, &o.old_name, &o.new_name);
        self.messages.insert(o.new_name.clone(), m);
        self.removed.insert(o.old_name.clone());
        self.touched.remove(&o.old_name);
        self.touch(&o.new_name);

        // Propagate the rename to every other decl in this SchemaObjects: field
        // `type_name`s and rpc input/output types that referenced the old name
        // (fully-qualified `.pkg.Old` or relative `Old`) now point at the new
        // name (design.md §7). Cross-file/repo propagation is out of scope.
        let touched: Vec<String> = self
            .messages
            .iter_mut()
            .filter(|(name, _)| name.as_str() != o.new_name)
            .filter_map(|(name, msg)| {
                rename_refs_in_message(msg, &o.old_name, &o.new_name).then(|| name.clone())
            })
            .collect();
        for name in touched {
            self.touch(&name);
        }

        let touched: Vec<String> = self
            .services
            .iter_mut()
            .filter_map(|(name, svc)| {
                rename_refs_in_service(svc, &o.old_name, &o.new_name).then(|| name.clone())
            })
            .collect();
        for name in touched {
            self.touch(&name);
        }
        Ok(())
    }

    fn delete_message(&mut self, o: &OpDeleteMessage) -> Result<(), MutationError> {
        if self.messages.remove(&o.message_name).is_none() {
            return Err(MutationError::DeclarationNotFound(o.message_name.clone()));
        }
        self.removed.insert(o.message_name.clone());
        self.touched.remove(&o.message_name);
        Ok(())
    }

    // ── Enum ops ────────────────────────────────────────────────────────────

    fn create_enum(&mut self, o: &OpCreateEnum) -> Result<(), MutationError> {
        if self.enums.contains_key(&o.enum_name) {
            return Err(MutationError::InvalidOperation(format!(
                "enum '{}' already exists",
                o.enum_name
            )));
        }
        self.enums.insert(
            o.enum_name.clone(),
            EnumDescriptorProto {
                name: Some(o.enum_name.clone()),
                ..Default::default()
            },
        );
        self.touch(&o.enum_name);
        Ok(())
    }

    fn delete_enum(&mut self, o: &OpDeleteEnum) -> Result<(), MutationError> {
        if self.enums.remove(&o.enum_name).is_none() {
            return Err(MutationError::DeclarationNotFound(o.enum_name.clone()));
        }
        self.removed.insert(o.enum_name.clone());
        self.touched.remove(&o.enum_name);
        Ok(())
    }

    fn add_enum_value(&mut self, o: &OpAddEnumValue) -> Result<(), MutationError> {
        let e = self.enum_mut(&o.enum_name)?;
        if e.value.iter().any(|v| v.name.as_deref() == Some(o.value_name.as_str())) {
            return Err(MutationError::InvalidOperation(format!(
                "enum value '{}' already exists",
                o.value_name
            )));
        }
        if e.value.iter().any(|v| v.number == Some(o.number)) {
            let allow_alias = e.options.as_ref().and_then(|x| x.allow_alias).unwrap_or(false);
            if !allow_alias {
                return Err(MutationError::InvalidOperation(format!(
                    "enum value number {} already in use (allow_alias not set)",
                    o.number
                )));
            }
        }
        e.value.push(EnumValueDescriptorProto {
            name: Some(o.value_name.clone()),
            number: Some(o.number),
            options: None,
            source_span: None,
        });
        self.touch(&o.enum_name);
        Ok(())
    }

    fn remove_enum_value(&mut self, o: &OpRemoveEnumValue) -> Result<(), MutationError> {
        let e = self.enum_mut(&o.enum_name)?;
        let idx = e
            .value
            .iter()
            .position(|v| v.name.as_deref() == Some(o.value_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.enum_name.clone(),
                field: o.value_name.clone(),
            })?;
        let removed = e.value.remove(idx);
        if let Some(num) = removed.number {
            e.reserved_range.push(protoc_rs_schema::EnumReservedRange {
                start: Some(num),
                end: Some(num),
            });
        }
        if let Some(name) = removed.name {
            e.reserved_name.push(name);
        }
        self.touch(&o.enum_name);
        Ok(())
    }

    fn rename_enum_value(&mut self, o: &OpRenameEnumValue) -> Result<(), MutationError> {
        let e = self.enum_mut(&o.enum_name)?;
        let v = e
            .value
            .iter_mut()
            .find(|v| v.name.as_deref() == Some(o.old_value_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.enum_name.clone(),
                field: o.old_value_name.clone(),
            })?;
        v.name = Some(o.new_value_name.clone());
        self.touch(&o.enum_name);
        Ok(())
    }

    // ── Service / RPC ops ─────────────────────────────────────────────────────

    fn add_service(&mut self, o: &OpAddService) -> Result<(), MutationError> {
        if self.services.contains_key(&o.service_name) {
            return Err(MutationError::InvalidOperation(format!(
                "service '{}' already exists",
                o.service_name
            )));
        }
        self.services.insert(
            o.service_name.clone(),
            ServiceDescriptorProto {
                name: Some(o.service_name.clone()),
                ..Default::default()
            },
        );
        self.touch(&o.service_name);
        Ok(())
    }

    fn remove_service(&mut self, o: &OpRemoveService) -> Result<(), MutationError> {
        if self.services.remove(&o.service_name).is_none() {
            return Err(MutationError::DeclarationNotFound(o.service_name.clone()));
        }
        self.removed.insert(o.service_name.clone());
        self.touched.remove(&o.service_name);
        Ok(())
    }

    fn rename_service(&mut self, o: &OpRenameService) -> Result<(), MutationError> {
        let mut s = self
            .services
            .remove(&o.old_name)
            .ok_or_else(|| MutationError::DeclarationNotFound(o.old_name.clone()))?;
        s.name = Some(o.new_name.clone());
        self.services.insert(o.new_name.clone(), s);
        self.removed.insert(o.old_name.clone());
        self.touched.remove(&o.old_name);
        self.touch(&o.new_name);
        Ok(())
    }

    fn add_rpc(&mut self, o: &OpAddRpc) -> Result<(), MutationError> {
        let s = self.service_mut(&o.service_name)?;
        if s.method.iter().any(|m| m.name.as_deref() == Some(o.rpc_name.as_str())) {
            return Err(MutationError::InvalidOperation(format!(
                "rpc '{}' already exists",
                o.rpc_name
            )));
        }
        s.method.push(MethodDescriptorProto {
            name: Some(o.rpc_name.clone()),
            input_type: Some(o.request_type.clone()),
            output_type: Some(o.response_type.clone()),
            options: None,
            client_streaming: Some(o.client_streaming),
            server_streaming: Some(o.server_streaming),
        });
        self.touch(&o.service_name);
        Ok(())
    }

    fn remove_rpc(&mut self, o: &OpRemoveRpc) -> Result<(), MutationError> {
        let s = self.service_mut(&o.service_name)?;
        let idx = s
            .method
            .iter()
            .position(|m| m.name.as_deref() == Some(o.rpc_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.service_name.clone(),
                field: o.rpc_name.clone(),
            })?;
        s.method.remove(idx);
        self.touch(&o.service_name);
        Ok(())
    }

    fn rename_rpc(&mut self, o: &OpRenameRpc) -> Result<(), MutationError> {
        let s = self.service_mut(&o.service_name)?;
        let m = s
            .method
            .iter_mut()
            .find(|m| m.name.as_deref() == Some(o.old_rpc_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.service_name.clone(),
                field: o.old_rpc_name.clone(),
            })?;
        m.name = Some(o.new_rpc_name.clone());
        self.touch(&o.service_name);
        Ok(())
    }

    fn change_rpc_type(&mut self, o: &OpChangeRpcType) -> Result<(), MutationError> {
        let s = self.service_mut(&o.service_name)?;
        let m = s
            .method
            .iter_mut()
            .find(|m| m.name.as_deref() == Some(o.rpc_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.service_name.clone(),
                field: o.rpc_name.clone(),
            })?;
        if !o.new_request_type.is_empty() {
            m.input_type = Some(o.new_request_type.clone());
        }
        if !o.new_response_type.is_empty() {
            m.output_type = Some(o.new_response_type.clone());
        }
        self.touch(&o.service_name);
        Ok(())
    }

    // ── Import op (file-level meta) ───────────────────────────────────────────

    fn update_import(&mut self, o: &OpUpdateImport) -> Result<(), MutationError> {
        if o.remove {
            if let Some(pos) = self.meta.dependency.iter().position(|d| d == &o.import_path) {
                self.meta.dependency.remove(pos);
                // Re-index public/weak dependency indices that pointed past pos.
                self.meta
                    .public_dependency
                    .retain(|&i| i != pos as i32);
                self.meta.weak_dependency.retain(|&i| i != pos as i32);
                for i in self.meta.public_dependency.iter_mut() {
                    if *i > pos as i32 {
                        *i -= 1;
                    }
                }
                for i in self.meta.weak_dependency.iter_mut() {
                    if *i > pos as i32 {
                        *i -= 1;
                    }
                }
            }
        } else if !self.meta.dependency.iter().any(|d| d == &o.import_path) {
            self.meta.dependency.push(o.import_path.clone());
        }
        self.meta_dirty = true;
        Ok(())
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn is_number_reserved(msg: &DescriptorProto, number: i32) -> bool {
    msg.reserved_range.iter().any(|r| {
        let start = r.start.unwrap_or(i32::MAX);
        let end = r.end.unwrap_or(i32::MIN); // exclusive
        number >= start && number < end
    })
}

/// Map a cardinality keyword to `(label, proto3_optional)`.
fn cardinality_to_label(
    card: &str,
) -> Result<(Option<FieldLabel>, bool), MutationError> {
    match card {
        "" | "singular" => Ok((None, false)),
        "optional" => Ok((Some(FieldLabel::Optional), true)),
        "required" => Ok((Some(FieldLabel::Required), false)),
        "repeated" => Ok((Some(FieldLabel::Repeated), false)),
        other => Err(MutationError::InvalidOperation(format!(
            "unknown cardinality '{other}'"
        ))),
    }
}

/// Resolve a type name to `(FieldType, type_name)`. Scalars get a `FieldType`
/// and no `type_name`; message/enum names get `type_name` with `r#type = None`
/// (the analyzer would later resolve it; for printing we keep the name).
fn resolve_type(name: &str) -> (Option<FieldType>, Option<String>) {
    match scalar_field_type(name) {
        Some(t) => (Some(t), None),
        None => (Some(FieldType::Message), Some(name.to_string())),
    }
}

/// If `type_ref`'s trailing `.`-segment equals `old`, return the reference with
/// that segment replaced by `new` (preserving any package qualifier and leading
/// dot). Returns `None` when the reference does not name `old`.
///
/// Handles both fully-qualified (`.pkg.Old`, `pkg.Old`) and relative (`Old`)
/// forms — protobuf type references can appear either way depending on whether
/// the analyzer has resolved them.
fn rename_type_ref(type_ref: &str, old: &str, new: &str) -> Option<String> {
    let (prefix, simple) = match type_ref.rfind('.') {
        Some(dot) => (&type_ref[..=dot], &type_ref[dot + 1..]),
        None => ("", type_ref),
    };
    if simple == old {
        Some(format!("{prefix}{new}"))
    } else {
        None
    }
}

/// Rewrite any field `type_name` in `msg` (and nested messages) that referenced
/// `old` to `new`. Returns whether anything changed.
fn rename_refs_in_message(msg: &mut DescriptorProto, old: &str, new: &str) -> bool {
    let mut changed = false;
    for f in &mut msg.field {
        if let Some(tn) = &f.type_name {
            if let Some(renamed) = rename_type_ref(tn, old, new) {
                f.type_name = Some(renamed);
                changed = true;
            }
        }
    }
    for nested in &mut msg.nested_type {
        changed |= rename_refs_in_message(nested, old, new);
    }
    changed
}

/// Rewrite any rpc input/output type in `svc` that referenced `old` to `new`.
/// Returns whether anything changed.
fn rename_refs_in_service(svc: &mut ServiceDescriptorProto, old: &str, new: &str) -> bool {
    let mut changed = false;
    for m in &mut svc.method {
        if let Some(t) = &m.input_type {
            if let Some(renamed) = rename_type_ref(t, old, new) {
                m.input_type = Some(renamed);
                changed = true;
            }
        }
        if let Some(t) = &m.output_type {
            if let Some(renamed) = rename_type_ref(t, old, new) {
                m.output_type = Some(renamed);
                changed = true;
            }
        }
    }
    changed
}

/// The current `.proto` type name of a field (scalar name or short type name).
fn current_type_name(field: &FieldDescriptorProto) -> String {
    match field.r#type {
        Some(FieldType::Message) | Some(FieldType::Enum) | Some(FieldType::Group) => field
            .type_name
            .clone()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_string(),
        Some(t) => t.proto_name().to_string(),
        None => field.type_name.clone().unwrap_or_default(),
    }
}
