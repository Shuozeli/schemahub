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
    let mut state = WorkingState::load(schema, false)?;
    state.apply(&parsed)?;
    Ok(state.into_effect(schema))
}

/// Apply an ordered batch; only the final state matters.
pub fn apply_many(
    schema: &SchemaObjects,
    ops: &[Mutation],
) -> Result<MutationEffect, MutationError> {
    let mut state = WorkingState::load(schema, true)?;
    for op in ops {
        let parsed = ProtoOp::decode(&op.operation)?;
        state.apply(&parsed)?;
    }
    state.validate_removed_references()?;
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
    removed_enum_values: std::collections::BTreeSet<(String, String)>,
    defer_reference_integrity: bool,
}

impl WorkingState {
    fn load(
        schema: &SchemaObjects,
        defer_reference_integrity: bool,
    ) -> Result<Self, MutationError> {
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
            removed_enum_values: Default::default(),
            defer_reference_integrity,
        })
    }

    fn into_effect(self, original: &SchemaObjects) -> MutationEffect {
        let mut upserts = Vec::new();
        for name in &self.touched {
            if self.removed.contains(name) {
                continue;
            }
            if let Some(m) = self.messages.get(name) {
                upserts.push((
                    name.clone(),
                    DeclBlob::new(encode_decl(&DeclPayload::Message(m.clone()))),
                ));
            } else if let Some(e) = self.enums.get(name) {
                upserts.push((
                    name.clone(),
                    DeclBlob::new(encode_decl(&DeclPayload::Enum(e.clone()))),
                ));
            } else if let Some(s) = self.services.get(name) {
                upserts.push((
                    name.clone(),
                    DeclBlob::new(encode_decl(&DeclPayload::Service(s.clone()))),
                ));
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

    fn declaration_exists(&self, name: &str) -> bool {
        self.messages.contains_key(name)
            || self.enums.contains_key(name)
            || self.services.contains_key(name)
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
        if self.declaration_exists(&o.message_name) {
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
        self.meta.message_order.push(o.message_name.clone());
        self.meta_dirty = true;
        self.touch(&o.message_name);
        Ok(())
    }

    fn rename_message(&mut self, o: &OpRenameMessage) -> Result<(), MutationError> {
        if o.old_name != o.new_name && self.declaration_exists(&o.new_name) {
            return Err(MutationError::InvalidOperation(format!(
                "cannot rename message '{}' to '{}': target already exists",
                o.old_name, o.new_name
            )));
        }
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

        let mut extension_changed = false;
        for field in &mut self.meta.extension {
            extension_changed |= rename_refs_in_field(field, &o.old_name, &o.new_name);
        }
        for name in &mut self.meta.message_order {
            if name == &o.old_name {
                *name = o.new_name.clone();
                extension_changed = true;
            }
        }
        if extension_changed {
            self.meta_dirty = true;
        }
        Ok(())
    }

    fn delete_message(&mut self, o: &OpDeleteMessage) -> Result<(), MutationError> {
        if !self.messages.contains_key(&o.message_name) {
            return Err(MutationError::DeclarationNotFound(o.message_name.clone()));
        }
        let references = self.references_to(&o.message_name, Some(&o.message_name));
        if !self.defer_reference_integrity && !references.is_empty() {
            return Err(referenced_declaration_error(
                "message",
                &o.message_name,
                &references,
            ));
        }
        self.messages.remove(&o.message_name);
        self.meta
            .message_order
            .retain(|name| name != &o.message_name);
        self.meta_dirty = true;
        self.removed.insert(o.message_name.clone());
        self.touched.remove(&o.message_name);
        Ok(())
    }

    // ── Enum ops ────────────────────────────────────────────────────────────

    fn create_enum(&mut self, o: &OpCreateEnum) -> Result<(), MutationError> {
        if self.declaration_exists(&o.enum_name) {
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
        self.meta.enum_order.push(o.enum_name.clone());
        self.meta_dirty = true;
        self.touch(&o.enum_name);
        Ok(())
    }

    fn delete_enum(&mut self, o: &OpDeleteEnum) -> Result<(), MutationError> {
        if !self.enums.contains_key(&o.enum_name) {
            return Err(MutationError::DeclarationNotFound(o.enum_name.clone()));
        }
        let references = self.references_to(&o.enum_name, None);
        if !self.defer_reference_integrity && !references.is_empty() {
            return Err(referenced_declaration_error(
                "enum",
                &o.enum_name,
                &references,
            ));
        }
        self.enums.remove(&o.enum_name);
        self.meta.enum_order.retain(|name| name != &o.enum_name);
        self.meta_dirty = true;
        self.removed.insert(o.enum_name.clone());
        self.touched.remove(&o.enum_name);
        Ok(())
    }

    fn add_enum_value(&mut self, o: &OpAddEnumValue) -> Result<(), MutationError> {
        let proto3 = self.meta.syntax.as_deref() == Some("proto3");
        let e = self.enum_mut(&o.enum_name)?;
        if e.value
            .iter()
            .any(|v| v.name.as_deref() == Some(o.value_name.as_str()))
        {
            return Err(MutationError::InvalidOperation(format!(
                "enum value '{}' already exists",
                o.value_name
            )));
        }
        if e.reserved_name.iter().any(|name| name == &o.value_name) {
            return Err(MutationError::InvalidOperation(format!(
                "enum value name '{}' is reserved",
                o.value_name
            )));
        }
        if is_enum_number_reserved(e, o.number) {
            return Err(MutationError::InvalidOperation(format!(
                "enum value number {} is reserved",
                o.number
            )));
        }
        if e.value.iter().any(|v| v.number == Some(o.number)) {
            let allow_alias = e
                .options
                .as_ref()
                .and_then(|x| x.allow_alias)
                .unwrap_or(false);
            if !allow_alias {
                return Err(MutationError::InvalidOperation(format!(
                    "enum value number {} already in use (allow_alias not set)",
                    o.number
                )));
            }
        }
        if proto3 && e.value.is_empty() && o.number != 0 {
            return Err(MutationError::InvalidOperation(format!(
                "the first value in proto3 enum '{}' must use number 0",
                o.enum_name
            )));
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
        let references = self.enum_value_references(&o.enum_name, &o.value_name);
        if !self.defer_reference_integrity && !references.is_empty() {
            return Err(referenced_enum_value_error(
                &o.enum_name,
                &o.value_name,
                &references,
            ));
        }
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
        self.removed_enum_values
            .insert((o.enum_name.clone(), o.value_name.clone()));
        self.touch(&o.enum_name);
        Ok(())
    }

    fn rename_enum_value(&mut self, o: &OpRenameEnumValue) -> Result<(), MutationError> {
        let e = self.enum_mut(&o.enum_name)?;
        if e.value
            .iter()
            .any(|v| v.name.as_deref() == Some(o.new_value_name.as_str()))
        {
            return Err(MutationError::InvalidOperation(format!(
                "enum value '{}' already exists",
                o.new_value_name
            )));
        }
        let v = e
            .value
            .iter_mut()
            .find(|v| v.name.as_deref() == Some(o.old_value_name.as_str()))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: o.enum_name.clone(),
                field: o.old_value_name.clone(),
            })?;
        v.name = Some(o.new_value_name.clone());
        let touched: Vec<String> = self
            .messages
            .iter_mut()
            .filter_map(|(name, message)| {
                rename_enum_value_refs_in_message(
                    message,
                    &o.enum_name,
                    &o.old_value_name,
                    &o.new_value_name,
                )
                .then(|| name.clone())
            })
            .collect();
        for name in touched {
            self.touch(&name);
        }
        let mut extension_changed = false;
        for field in &mut self.meta.extension {
            extension_changed |= rename_enum_value_ref_in_field(
                field,
                &o.enum_name,
                &o.old_value_name,
                &o.new_value_name,
            );
        }
        if extension_changed {
            self.meta_dirty = true;
        }
        self.touch(&o.enum_name);
        Ok(())
    }

    // ── Service / RPC ops ─────────────────────────────────────────────────────

    fn add_service(&mut self, o: &OpAddService) -> Result<(), MutationError> {
        if self.declaration_exists(&o.service_name) {
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
        self.meta.service_order.push(o.service_name.clone());
        self.meta_dirty = true;
        self.touch(&o.service_name);
        Ok(())
    }

    fn remove_service(&mut self, o: &OpRemoveService) -> Result<(), MutationError> {
        if self.services.remove(&o.service_name).is_none() {
            return Err(MutationError::DeclarationNotFound(o.service_name.clone()));
        }
        self.meta
            .service_order
            .retain(|name| name != &o.service_name);
        self.meta_dirty = true;
        self.removed.insert(o.service_name.clone());
        self.touched.remove(&o.service_name);
        Ok(())
    }

    fn rename_service(&mut self, o: &OpRenameService) -> Result<(), MutationError> {
        if o.old_name != o.new_name && self.declaration_exists(&o.new_name) {
            return Err(MutationError::InvalidOperation(format!(
                "cannot rename service '{}' to '{}': target already exists",
                o.old_name, o.new_name
            )));
        }
        let mut s = self
            .services
            .remove(&o.old_name)
            .ok_or_else(|| MutationError::DeclarationNotFound(o.old_name.clone()))?;
        s.name = Some(o.new_name.clone());
        self.services.insert(o.new_name.clone(), s);
        for name in &mut self.meta.service_order {
            if name == &o.old_name {
                *name = o.new_name.clone();
            }
        }
        self.meta_dirty = true;
        self.removed.insert(o.old_name.clone());
        self.touched.remove(&o.old_name);
        self.touch(&o.new_name);
        Ok(())
    }

    fn add_rpc(&mut self, o: &OpAddRpc) -> Result<(), MutationError> {
        let s = self.service_mut(&o.service_name)?;
        if s.method
            .iter()
            .any(|m| m.name.as_deref() == Some(o.rpc_name.as_str()))
        {
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
        if s.method
            .iter()
            .any(|method| method.name.as_deref() == Some(o.new_rpc_name.as_str()))
        {
            return Err(MutationError::InvalidOperation(format!(
                "rpc '{}' already exists",
                o.new_rpc_name
            )));
        }
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
        self.meta
            .dependency_commit
            .resize(self.meta.dependency.len(), String::new());
        if o.remove {
            if let Some(pos) = self
                .meta
                .dependency
                .iter()
                .position(|d| d == &o.import_path)
            {
                self.meta.dependency.remove(pos);
                self.meta.dependency_commit.remove(pos);
                // Re-index public/weak dependency indices that pointed past pos.
                self.meta.public_dependency.retain(|&i| i != pos as i32);
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
        } else if let Some(pos) = self
            .meta
            .dependency
            .iter()
            .position(|dependency| dependency == &o.import_path)
        {
            self.meta.dependency_commit[pos] = o.resolved_commit.clone();
        } else {
            self.meta.dependency.push(o.import_path.clone());
            self.meta.dependency_commit.push(o.resolved_commit.clone());
        }
        self.meta_dirty = true;
        Ok(())
    }

    fn references_to(&self, target: &str, skip_message: Option<&str>) -> Vec<String> {
        let mut references = Vec::new();
        for (name, message) in &self.messages {
            if skip_message == Some(name.as_str()) {
                continue;
            }
            if message_references_type(message, target) {
                references.push(name.clone());
            }
        }
        for (name, service) in &self.services {
            if service_references_type(service, target) {
                references.push(name.clone());
            }
        }
        if self
            .meta
            .extension
            .iter()
            .any(|field| field_references_type(field, target))
        {
            references.push("file extension".to_string());
        }
        references
    }

    fn validate_removed_references(&self) -> Result<(), MutationError> {
        for name in &self.removed {
            let references = self.references_to(name, Some(name));
            if !references.is_empty() {
                return Err(referenced_declaration_error(
                    "declaration",
                    name,
                    &references,
                ));
            }
        }
        for (enum_name, value_name) in &self.removed_enum_values {
            let references = self.enum_value_references(enum_name, value_name);
            if !references.is_empty() {
                return Err(referenced_enum_value_error(
                    enum_name,
                    value_name,
                    &references,
                ));
            }
        }
        Ok(())
    }

    fn enum_value_references(&self, enum_name: &str, value_name: &str) -> Vec<String> {
        let mut references: Vec<String> = self
            .messages
            .iter()
            .filter(|(_, message)| message_references_enum_value(message, enum_name, value_name))
            .map(|(name, _)| name.clone())
            .collect();
        if self
            .meta
            .extension
            .iter()
            .any(|field| field_references_enum_value(field, enum_name, value_name))
        {
            references.push("file extension".to_string());
        }
        references
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

fn is_enum_number_reserved(enumeration: &EnumDescriptorProto, number: i32) -> bool {
    enumeration.reserved_range.iter().any(|range| {
        let start = range.start.unwrap_or(i32::MAX);
        let end = range.end.unwrap_or(i32::MIN);
        number >= start && number <= end
    })
}

/// Map a cardinality keyword to `(label, proto3_optional)`.
fn cardinality_to_label(card: &str) -> Result<(Option<FieldLabel>, bool), MutationError> {
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

fn type_ref_matches(type_ref: &str, target: &str) -> bool {
    type_ref
        .rsplit('.')
        .next()
        .is_some_and(|simple| simple == target)
}

fn message_references_type(message: &DescriptorProto, target: &str) -> bool {
    message
        .field
        .iter()
        .chain(&message.extension)
        .any(|field| field_references_type(field, target))
        || message
            .nested_type
            .iter()
            .any(|nested| message_references_type(nested, target))
}

fn field_references_type(field: &FieldDescriptorProto, target: &str) -> bool {
    [&field.type_name, &field.extendee]
        .into_iter()
        .flatten()
        .any(|name| type_ref_matches(name, target))
}

fn field_references_enum_value(
    field: &FieldDescriptorProto,
    enum_name: &str,
    value_name: &str,
) -> bool {
    field
        .type_name
        .as_deref()
        .is_some_and(|name| type_ref_matches(name, enum_name))
        && field.default_value.as_deref() == Some(value_name)
}

fn message_references_enum_value(
    message: &DescriptorProto,
    enum_name: &str,
    value_name: &str,
) -> bool {
    message
        .field
        .iter()
        .chain(&message.extension)
        .any(|field| field_references_enum_value(field, enum_name, value_name))
        || message
            .nested_type
            .iter()
            .any(|nested| message_references_enum_value(nested, enum_name, value_name))
}

fn service_references_type(service: &ServiceDescriptorProto, target: &str) -> bool {
    service.method.iter().any(|method| {
        method
            .input_type
            .as_deref()
            .is_some_and(|name| type_ref_matches(name, target))
            || method
                .output_type
                .as_deref()
                .is_some_and(|name| type_ref_matches(name, target))
    })
}

fn referenced_declaration_error(kind: &str, target: &str, references: &[String]) -> MutationError {
    MutationError::InvalidOperation(format!(
        "cannot delete {kind} '{target}': referenced by {}",
        references.join(", ")
    ))
}

fn referenced_enum_value_error(
    enum_name: &str,
    value_name: &str,
    references: &[String],
) -> MutationError {
    MutationError::InvalidOperation(format!(
        "cannot remove enum value '{enum_name}.{value_name}': referenced as a default by {}",
        references.join(", ")
    ))
}

/// Rewrite any field `type_name` in `msg` (and nested messages) that referenced
/// `old` to `new`. Returns whether anything changed.
fn rename_refs_in_message(msg: &mut DescriptorProto, old: &str, new: &str) -> bool {
    let mut changed = false;
    for field in msg.field.iter_mut().chain(&mut msg.extension) {
        changed |= rename_refs_in_field(field, old, new);
    }
    for nested in &mut msg.nested_type {
        changed |= rename_refs_in_message(nested, old, new);
    }
    changed
}

fn rename_refs_in_field(field: &mut FieldDescriptorProto, old: &str, new: &str) -> bool {
    let mut changed = false;
    for reference in [&mut field.type_name, &mut field.extendee]
        .into_iter()
        .flatten()
    {
        if let Some(renamed) = rename_type_ref(reference, old, new) {
            *reference = renamed;
            changed = true;
        }
    }
    changed
}

fn rename_enum_value_refs_in_message(
    message: &mut DescriptorProto,
    enum_name: &str,
    old_value_name: &str,
    new_value_name: &str,
) -> bool {
    let mut changed = false;
    for field in message.field.iter_mut().chain(&mut message.extension) {
        changed |= rename_enum_value_ref_in_field(field, enum_name, old_value_name, new_value_name);
    }
    for nested in &mut message.nested_type {
        changed |=
            rename_enum_value_refs_in_message(nested, enum_name, old_value_name, new_value_name);
    }
    changed
}

fn rename_enum_value_ref_in_field(
    field: &mut FieldDescriptorProto,
    enum_name: &str,
    old_value_name: &str,
    new_value_name: &str,
) -> bool {
    if field_references_enum_value(field, enum_name, old_value_name) {
        field.default_value = Some(new_value_name.to_string());
        true
    } else {
        false
    }
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
