//! Typed mutation operations and their application against the real `Schema`
//! (design.md §3.2 / v1 §4.3).
//!
//! The op enum is serialized with `serde_json` into the opaque
//! [`Mutation::operation`] bytes; the compiler is the only component that
//! decodes them.
//!
//! Constraints (enforced here):
//!   - `AddField` appends at the end of a table only.
//!   - `RemoveField` is rejected — callers must `DeprecateField` instead.
//!   - Any struct mutation is rejected (fixed layout).
//!   - Field reorder is rejected.

use serde::{Deserialize, Serialize};

use flatc_rs_schema::{Attributes, BaseType, Enum, EnumVal, Field, KeyValue, Object, Type};
use schemahub_types::{Mutation, MutationEffect, MutationError, SchemaObjects};

use crate::blob::{decode_decl, decode_meta, encode_decl, encode_meta, DeclPayload, FbsMeta};

/// A typed FlatBuffers mutation operation.
///
/// (De)serialized from [`Mutation::operation`] with `serde_json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FbsOp {
    /// Append a field to the end of a table. Type is the canonical `.fbs`
    /// spelling (e.g. `"string"`, `"int"`, `"Foo"`). Rejected for structs.
    AddField {
        table: String,
        field_name: String,
        field_type: String,
        default_value: Option<String>,
        doc_comment: Option<String>,
    },
    /// Mark a field `(deprecated)`. The slot is retained for wire compatibility.
    DeprecateField { table: String, field_name: String },
    /// Rename a field (does not change its slot/wire identity).
    RenameField {
        table: String,
        old_name: String,
        new_name: String,
    },
    /// Change a field's type. Always breaking at compat time, but a valid edit.
    ChangeFieldType {
        table: String,
        field_name: String,
        new_type: String,
    },
    /// Remove a field. Always REJECTED — use `DeprecateField`.
    RemoveField { table: String, field_name: String },

    /// Create a new (empty) table.
    CreateTable {
        name: String,
        doc_comment: Option<String>,
    },
    /// Rename a table.
    RenameTable { old_name: String, new_name: String },
    /// Delete a table.
    DeleteTable { name: String },

    /// Create a new enum with the given underlying scalar type (e.g. `"int"`).
    CreateEnum {
        name: String,
        underlying_type: String,
        doc_comment: Option<String>,
    },
    /// Rename an enum.
    RenameEnum { old_name: String, new_name: String },
    /// Delete an enum.
    DeleteEnum { name: String },
    /// Add a value to an enum.
    AddEnumValue {
        enum_name: String,
        value_name: String,
        value: i64,
    },
    /// Remove a value from an enum.
    RemoveEnumValue {
        enum_name: String,
        value_name: String,
    },
    /// Rename a value in an enum.
    RenameEnumValue {
        enum_name: String,
        old_name: String,
        new_name: String,
    },

    /// Create a new (empty) union.
    CreateUnion {
        name: String,
        doc_comment: Option<String>,
    },
    /// Create a union and populate its initial table members atomically.
    CreateUnionWithMembers {
        name: String,
        member_types: Vec<String>,
        doc_comment: Option<String>,
    },
    /// Add one table member to a union using the next discriminator value.
    AddUnionMember {
        union_name: String,
        member_type: String,
    },
    /// Remove one table member without renumbering the remaining values.
    RemoveUnionMember {
        union_name: String,
        member_type: String,
    },
    /// Rename a union.
    RenameUnion { old_name: String, new_name: String },
    /// Delete a union.
    DeleteUnion { name: String },

    /// Add or remove an `include "..."` in the file metadata.
    UpdateImport {
        import_path: String,
        resolved_commit: String,
        remove: bool,
    },
}

impl FbsOp {
    /// Encode this op into the opaque mutation operation bytes.
    pub fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(serde_json::to_vec(self).expect("FbsOp serializes"))
    }

    /// Decode an op from opaque mutation operation bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, MutationError> {
        serde_json::from_slice(bytes)
            .map_err(|e| MutationError::InvalidOperationBytes(e.to_string()))
    }
}

/// Apply a single mutation, returning its effect.
pub fn apply_mutation(
    schema: &SchemaObjects,
    op: &Mutation,
) -> Result<MutationEffect, MutationError> {
    let parsed = FbsOp::decode(&op.operation)?;
    let mut state = WorkingState::load(schema, false)?;
    state.apply(&parsed)?;
    Ok(state.into_effect())
}

/// Apply an ordered batch of mutations; only the final state matters.
pub fn apply_mutations(
    schema: &SchemaObjects,
    ops: &[Mutation],
) -> Result<MutationEffect, MutationError> {
    let mut state = WorkingState::load(schema, true)?;
    for op in ops {
        let parsed = FbsOp::decode(&op.operation)?;
        state.apply(&parsed)?;
    }
    state.validate_removed_references()?;
    Ok(state.into_effect())
}

/// Mutable working copy of a schema's declarations plus change tracking.
struct WorkingState {
    /// Live, name-keyed decl payloads (mutated in place).
    decls: std::collections::BTreeMap<String, DeclPayload>,
    meta: FbsMeta,
    meta_changed: bool,
    /// Names that have been touched (upserted).
    touched: std::collections::BTreeSet<String>,
    /// Names removed.
    removed: std::collections::BTreeSet<String>,
    removed_enum_values: std::collections::BTreeSet<(String, String)>,
    defer_reference_integrity: bool,
}

#[derive(Clone, Copy)]
enum ExpectedDeclKind {
    Table,
    Enum,
    Union,
}

impl ExpectedDeclKind {
    fn label(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Enum => "enum",
            Self::Union => "union",
        }
    }
}

impl WorkingState {
    fn load(
        schema: &SchemaObjects,
        defer_reference_integrity: bool,
    ) -> Result<Self, MutationError> {
        let mut decls = std::collections::BTreeMap::new();
        for (name, blob) in &schema.decls {
            let payload =
                decode_decl(blob).map_err(|e| MutationError::MalformedBlob(e.to_string()))?;
            decls.insert(name.clone(), payload);
        }
        let meta =
            decode_meta(&schema.meta).map_err(|e| MutationError::MalformedBlob(e.to_string()))?;
        Ok(Self {
            decls,
            meta,
            meta_changed: false,
            touched: Default::default(),
            removed: Default::default(),
            removed_enum_values: Default::default(),
            defer_reference_integrity,
        })
    }

    fn into_effect(self) -> MutationEffect {
        let mut upserts = Vec::new();
        for name in &self.touched {
            if let Some(payload) = self.decls.get(name) {
                upserts.push((name.clone(), encode_decl(payload.clone())));
            }
        }
        let removes = self
            .removed
            .into_iter()
            .filter(|name| !self.decls.contains_key(name))
            .collect();
        MutationEffect {
            meta: if self.meta_changed {
                Some(encode_meta(self.meta))
            } else {
                None
            },
            upserts,
            removes,
        }
    }

    fn object_mut(&mut self, name: &str) -> Result<&mut Object, MutationError> {
        match self.decls.get_mut(name) {
            Some(DeclPayload::Object(o)) => Ok(o),
            Some(_) => Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' is not a table/struct"
            ))),
            None => Err(MutationError::DeclarationNotFound(name.to_string())),
        }
    }

    fn enum_mut(&mut self, name: &str) -> Result<&mut Enum, MutationError> {
        match self.decls.get_mut(name) {
            Some(DeclPayload::Enum(e)) => Ok(e),
            Some(_) => Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' is not an enum/union"
            ))),
            None => Err(MutationError::DeclarationNotFound(name.to_string())),
        }
    }

    fn regular_enum_mut(&mut self, name: &str) -> Result<&mut Enum, MutationError> {
        let declaration = self.enum_mut(name)?;
        if declaration.is_union {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' is a union, not an enum"
            )));
        }
        Ok(declaration)
    }

    fn union_mut(&mut self, name: &str) -> Result<&mut Enum, MutationError> {
        let declaration = self.enum_mut(name)?;
        if !declaration.is_union {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' is an enum, not a union"
            )));
        }
        Ok(declaration)
    }

    fn apply(&mut self, op: &FbsOp) -> Result<(), MutationError> {
        match op {
            FbsOp::AddField {
                table,
                field_name,
                field_type,
                default_value,
                doc_comment,
            } => self.add_field(table, field_name, field_type, default_value, doc_comment),
            FbsOp::DeprecateField { table, field_name } => self.deprecate_field(table, field_name),
            FbsOp::RenameField {
                table,
                old_name,
                new_name,
            } => self.rename_field(table, old_name, new_name),
            FbsOp::ChangeFieldType {
                table,
                field_name,
                new_type,
            } => self.change_field_type(table, field_name, new_type),
            FbsOp::RemoveField { table, .. } => Err(MutationError::InvalidOperation(format!(
                "RemoveField on '{table}' is rejected in FlatBuffers; use DeprecateField (slot is wire identity)"
            ))),

            FbsOp::CreateTable { name, doc_comment } => self.create_table(name, doc_comment),
            FbsOp::RenameTable { old_name, new_name } => {
                self.rename_decl(old_name, new_name, ExpectedDeclKind::Table)
            }
            FbsOp::DeleteTable { name } => self.delete_decl(name, ExpectedDeclKind::Table),

            FbsOp::CreateEnum {
                name,
                underlying_type,
                doc_comment,
            } => self.create_enum(name, underlying_type, doc_comment, false),
            FbsOp::RenameEnum { old_name, new_name } => {
                self.rename_decl(old_name, new_name, ExpectedDeclKind::Enum)
            }
            FbsOp::DeleteEnum { name } => self.delete_decl(name, ExpectedDeclKind::Enum),
            FbsOp::AddEnumValue {
                enum_name,
                value_name,
                value,
            } => self.add_enum_value(enum_name, value_name, *value),
            FbsOp::RemoveEnumValue {
                enum_name,
                value_name,
            } => self.remove_enum_value(enum_name, value_name),
            FbsOp::RenameEnumValue {
                enum_name,
                old_name,
                new_name,
            } => self.rename_enum_value(enum_name, old_name, new_name),

            FbsOp::CreateUnion { name, doc_comment } => self.create_union(name, doc_comment),
            FbsOp::CreateUnionWithMembers {
                name,
                member_types,
                doc_comment,
            } => self.create_union_with_members(name, member_types, doc_comment),
            FbsOp::AddUnionMember {
                union_name,
                member_type,
            } => self.add_union_member(union_name, member_type),
            FbsOp::RemoveUnionMember {
                union_name,
                member_type,
            } => self.remove_union_member(union_name, member_type),
            FbsOp::RenameUnion { old_name, new_name } => {
                self.rename_decl(old_name, new_name, ExpectedDeclKind::Union)
            }
            FbsOp::DeleteUnion { name } => self.delete_decl(name, ExpectedDeclKind::Union),

            FbsOp::UpdateImport {
                import_path,
                resolved_commit,
                remove,
            } => self.update_import(import_path, resolved_commit, *remove),
        }
    }

    // ── Table field operations ───────────────────────────────────────────────

    fn add_field(
        &mut self,
        table: &str,
        field_name: &str,
        field_type: &str,
        default_value: &Option<String>,
        doc_comment: &Option<String>,
    ) -> Result<(), MutationError> {
        let obj = self.object_mut(table)?;
        if obj.is_struct {
            return Err(MutationError::InvalidOperation(format!(
                "cannot add a field to struct '{table}': struct layout is fixed"
            )));
        }
        if obj
            .fields
            .iter()
            .any(|f| f.name.as_deref() == Some(field_name))
        {
            return Err(MutationError::InvalidOperation(format!(
                "field '{field_name}' already exists in '{table}'"
            )));
        }
        // Append at the end: assign the next slot id.
        let next_slot = obj
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| f.id.unwrap_or(i as u32))
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);

        let mut field = Field {
            name: Some(field_name.to_string()),
            type_: Some(parse_type_spec(field_type)),
            id: Some(next_slot),
            ..Default::default()
        };
        if let Some(dv) = default_value {
            // Store as integer/real/string heuristically.
            if let Ok(i) = dv.parse::<i64>() {
                field.default_integer = Some(i);
            } else if let Ok(r) = dv.parse::<f64>() {
                field.default_real = Some(r);
            } else {
                field.default_string = Some(dv.clone());
            }
        }
        if let Some(doc) = doc_comment {
            field.documentation = Some(flatc_rs_schema::Documentation {
                lines: vec![doc.clone()],
            });
        }
        obj.fields.push(field);
        self.touched.insert(table.to_string());
        Ok(())
    }

    fn deprecate_field(&mut self, table: &str, field_name: &str) -> Result<(), MutationError> {
        let obj = self.object_mut(table)?;
        if obj.is_struct {
            return Err(MutationError::InvalidOperation(format!(
                "cannot deprecate a field on struct '{table}'"
            )));
        }
        let field = obj
            .fields
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(field_name))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: table.to_string(),
                field: field_name.to_string(),
            })?;
        field.is_deprecated = true;
        set_flag_attribute(field, "deprecated");
        self.touched.insert(table.to_string());
        Ok(())
    }

    fn rename_field(
        &mut self,
        table: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), MutationError> {
        let obj = self.object_mut(table)?;
        if obj.is_struct {
            return Err(MutationError::InvalidOperation(format!(
                "cannot rename a field on struct '{table}': struct layout is fixed"
            )));
        }
        if obj
            .fields
            .iter()
            .any(|field| field.name.as_deref() == Some(new_name))
        {
            return Err(MutationError::InvalidOperation(format!(
                "field '{new_name}' already exists in '{table}'"
            )));
        }
        let field = obj
            .fields
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(old_name))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: table.to_string(),
                field: old_name.to_string(),
            })?;
        field.name = Some(new_name.to_string());
        self.touched.insert(table.to_string());
        Ok(())
    }

    fn change_field_type(
        &mut self,
        table: &str,
        field_name: &str,
        new_type: &str,
    ) -> Result<(), MutationError> {
        let obj = self.object_mut(table)?;
        if obj.is_struct {
            return Err(MutationError::InvalidOperation(format!(
                "cannot change a field type on struct '{table}'"
            )));
        }
        let field = obj
            .fields
            .iter_mut()
            .find(|f| f.name.as_deref() == Some(field_name))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: table.to_string(),
                field: field_name.to_string(),
            })?;
        field.type_ = Some(parse_type_spec(new_type));
        self.touched.insert(table.to_string());
        Ok(())
    }

    // ── Declaration create / rename / delete ──────────────────────────────────

    fn create_table(
        &mut self,
        name: &str,
        doc_comment: &Option<String>,
    ) -> Result<(), MutationError> {
        if self.decls.contains_key(name) {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' already exists"
            )));
        }
        let obj = Object {
            name: Some(name.to_string()),
            documentation: doc_comment
                .as_ref()
                .map(|d| flatc_rs_schema::Documentation {
                    lines: vec![d.clone()],
                }),
            ..Default::default()
        };
        self.decls
            .insert(name.to_string(), DeclPayload::Object(Box::new(obj)));
        self.meta.decl_order.push(name.to_string());
        self.meta_changed = true;
        self.touched.insert(name.to_string());
        Ok(())
    }

    fn create_enum(
        &mut self,
        name: &str,
        underlying_type: &str,
        doc_comment: &Option<String>,
        is_union: bool,
    ) -> Result<(), MutationError> {
        if self.decls.contains_key(name) {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' already exists"
            )));
        }
        let underlying = parse_type_spec(underlying_type);
        let en = Enum {
            name: Some(name.to_string()),
            is_union,
            underlying_type: Some(underlying),
            documentation: doc_comment
                .as_ref()
                .map(|d| flatc_rs_schema::Documentation {
                    lines: vec![d.clone()],
                }),
            ..Default::default()
        };
        self.decls
            .insert(name.to_string(), DeclPayload::Enum(Box::new(en)));
        self.meta.decl_order.push(name.to_string());
        self.meta_changed = true;
        self.touched.insert(name.to_string());
        Ok(())
    }

    fn create_union(
        &mut self,
        name: &str,
        doc_comment: &Option<String>,
    ) -> Result<(), MutationError> {
        if self.decls.contains_key(name) {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' already exists"
            )));
        }
        // Unions default to a uint8 (`UType`) discriminator and a NONE sentinel,
        // matching the parser's representation.
        let utype = Type {
            base_type: Some(BaseType::BASE_TYPE_U_TYPE),
            ..Default::default()
        };
        let none = EnumVal {
            name: Some("NONE".to_string()),
            value: Some(0),
            ..Default::default()
        };
        let en = Enum {
            name: Some(name.to_string()),
            is_union: true,
            underlying_type: Some(utype),
            values: vec![none],
            documentation: doc_comment
                .as_ref()
                .map(|d| flatc_rs_schema::Documentation {
                    lines: vec![d.clone()],
                }),
            ..Default::default()
        };
        self.decls
            .insert(name.to_string(), DeclPayload::Enum(Box::new(en)));
        self.meta.decl_order.push(name.to_string());
        self.meta_changed = true;
        self.touched.insert(name.to_string());
        Ok(())
    }

    fn create_union_with_members(
        &mut self,
        name: &str,
        member_types: &[String],
        doc_comment: &Option<String>,
    ) -> Result<(), MutationError> {
        self.create_union(name, doc_comment)?;
        for member_type in member_types {
            self.add_union_member(name, member_type)?;
        }
        Ok(())
    }

    fn add_union_member(
        &mut self,
        union_name: &str,
        member_type: &str,
    ) -> Result<(), MutationError> {
        let base_type = match self.decls.get(member_type) {
            Some(DeclPayload::Object(object)) if !object.is_struct => BaseType::BASE_TYPE_TABLE,
            Some(DeclPayload::Object(_)) => {
                return Err(MutationError::InvalidOperation(format!(
                    "union member '{member_type}' must be a table, not a struct"
                )))
            }
            Some(_) => {
                return Err(MutationError::InvalidOperation(format!(
                    "union member '{member_type}' is not a table"
                )))
            }
            None => return Err(MutationError::DeclarationNotFound(member_type.to_string())),
        };
        let union = self.union_mut(union_name)?;
        if union.values.iter().any(|value| {
            value.name.as_deref() == Some(member_type)
                || value
                    .union_type
                    .as_ref()
                    .and_then(|member| member.unresolved_name.as_deref())
                    .is_some_and(|name| type_name_matches(name, member_type))
        }) {
            return Err(MutationError::InvalidOperation(format!(
                "union member '{member_type}' already exists in '{union_name}'"
            )));
        }
        let next_value = union
            .values
            .iter()
            .filter_map(|value| value.value)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                MutationError::InvalidOperation(format!(
                    "union '{union_name}' has no remaining discriminator values"
                ))
            })?;
        union.values.push(EnumVal {
            name: Some(member_type.to_string()),
            value: Some(next_value),
            union_type: Some(Type {
                base_type: Some(base_type),
                base_size: Some(4),
                unresolved_name: Some(member_type.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        self.touched.insert(union_name.to_string());
        Ok(())
    }

    fn remove_union_member(
        &mut self,
        union_name: &str,
        member_type: &str,
    ) -> Result<(), MutationError> {
        if member_type == "NONE" {
            return Err(MutationError::InvalidOperation(
                "the synthetic NONE union member cannot be removed".to_string(),
            ));
        }
        let union = self.union_mut(union_name)?;
        let index = union
            .values
            .iter()
            .position(|value| {
                value.name.as_deref() == Some(member_type)
                    || value
                        .union_type
                        .as_ref()
                        .and_then(|member| member.unresolved_name.as_deref())
                        .is_some_and(|name| type_name_matches(name, member_type))
            })
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: union_name.to_string(),
                field: member_type.to_string(),
            })?;
        union.values.remove(index);
        self.touched.insert(union_name.to_string());
        Ok(())
    }

    fn rename_decl(
        &mut self,
        old_name: &str,
        new_name: &str,
        expected: ExpectedDeclKind,
    ) -> Result<(), MutationError> {
        let payload = self
            .decls
            .get(old_name)
            .ok_or_else(|| MutationError::DeclarationNotFound(old_name.to_string()))?;
        if !payload_matches_kind(payload, expected) {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{old_name}' is not a {}",
                expected.label()
            )));
        }
        if old_name == new_name {
            return Ok(());
        }
        if self.decls.contains_key(new_name) {
            return Err(MutationError::InvalidOperation(format!(
                "cannot rename '{old_name}' to '{new_name}': target already exists"
            )));
        }
        let mut payload = self
            .decls
            .remove(old_name)
            .expect("declaration existence checked above");
        match &mut payload {
            DeclPayload::Object(o) => o.name = Some(new_name.to_string()),
            DeclPayload::Enum(e) => e.name = Some(new_name.to_string()),
            DeclPayload::Service(s) => s.name = Some(new_name.to_string()),
        }
        self.decls.insert(new_name.to_string(), payload);

        let changed_references: Vec<String> = self
            .decls
            .iter_mut()
            .filter_map(|(name, payload)| {
                rewrite_payload_references(payload, old_name, new_name).then(|| name.clone())
            })
            .collect();
        self.touched.extend(changed_references);

        if let Some(root_type) = &mut self.meta.root_type {
            if let Some(renamed) = rename_type_name(root_type, old_name, new_name) {
                *root_type = renamed;
                self.meta_changed = true;
            }
        }
        for name in &mut self.meta.decl_order {
            if name == old_name {
                *name = new_name.to_string();
                self.meta_changed = true;
            }
        }
        self.removed.insert(old_name.to_string());
        self.touched.remove(old_name);
        self.touched.insert(new_name.to_string());
        Ok(())
    }

    fn delete_decl(&mut self, name: &str, expected: ExpectedDeclKind) -> Result<(), MutationError> {
        let payload = self
            .decls
            .get(name)
            .ok_or_else(|| MutationError::DeclarationNotFound(name.to_string()))?;
        if !payload_matches_kind(payload, expected) {
            return Err(MutationError::InvalidOperation(format!(
                "declaration '{name}' is not a {}",
                expected.label()
            )));
        }
        let mut references: Vec<String> = self
            .decls
            .iter()
            .filter(|(declaration_name, _)| declaration_name.as_str() != name)
            .filter(|(_, payload)| payload_references(payload, name))
            .map(|(declaration_name, _)| declaration_name.clone())
            .collect();
        if self
            .meta
            .root_type
            .as_deref()
            .is_some_and(|root| type_name_matches(root, name))
        {
            references.push("root_type".to_string());
        }
        if !self.defer_reference_integrity && !references.is_empty() {
            return Err(MutationError::InvalidOperation(format!(
                "cannot delete {} '{name}': referenced by {}",
                expected.label(),
                references.join(", ")
            )));
        }
        self.decls.remove(name);
        self.meta
            .decl_order
            .retain(|declaration| declaration != name);
        self.meta_changed = true;
        self.touched.remove(name);
        self.removed.insert(name.to_string());
        Ok(())
    }

    // ── Enum value operations ──────────────────────────────────────────────────

    fn add_enum_value(
        &mut self,
        enum_name: &str,
        value_name: &str,
        value: i64,
    ) -> Result<(), MutationError> {
        let en = self.regular_enum_mut(enum_name)?;
        if en
            .values
            .iter()
            .any(|v| v.name.as_deref() == Some(value_name))
        {
            return Err(MutationError::InvalidOperation(format!(
                "enum value '{value_name}' already exists in '{enum_name}'"
            )));
        }
        if en
            .values
            .iter()
            .any(|existing| existing.value == Some(value))
        {
            return Err(MutationError::InvalidOperation(format!(
                "enum value number '{value}' already exists in '{enum_name}'"
            )));
        }
        en.values.push(EnumVal {
            name: Some(value_name.to_string()),
            value: Some(value),
            ..Default::default()
        });
        self.removed_enum_values
            .remove(&(enum_name.to_string(), value_name.to_string()));
        self.touched.insert(enum_name.to_string());
        Ok(())
    }

    fn remove_enum_value(
        &mut self,
        enum_name: &str,
        value_name: &str,
    ) -> Result<(), MutationError> {
        let references = self.enum_value_references(enum_name, value_name);
        if !self.defer_reference_integrity && !references.is_empty() {
            return Err(MutationError::InvalidOperation(format!(
                "cannot remove enum value '{enum_name}.{value_name}': referenced as a default by {}",
                references.join(", ")
            )));
        }
        let en = self.regular_enum_mut(enum_name)?;
        let before = en.values.len();
        en.values.retain(|v| v.name.as_deref() != Some(value_name));
        if en.values.len() == before {
            return Err(MutationError::FieldNotFound {
                declaration: enum_name.to_string(),
                field: value_name.to_string(),
            });
        }
        self.removed_enum_values
            .insert((enum_name.to_string(), value_name.to_string()));
        self.touched.insert(enum_name.to_string());
        Ok(())
    }

    fn rename_enum_value(
        &mut self,
        enum_name: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), MutationError> {
        let en = self.regular_enum_mut(enum_name)?;
        if en
            .values
            .iter()
            .any(|value| value.name.as_deref() == Some(new_name))
        {
            return Err(MutationError::InvalidOperation(format!(
                "enum value '{new_name}' already exists in '{enum_name}'"
            )));
        }
        let val = en
            .values
            .iter_mut()
            .find(|v| v.name.as_deref() == Some(old_name))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: enum_name.to_string(),
                field: old_name.to_string(),
            })?;
        val.name = Some(new_name.to_string());
        let changed_defaults: Vec<String> = self
            .decls
            .iter_mut()
            .filter_map(|(name, payload)| match payload {
                DeclPayload::Object(object) => {
                    let mut changed = false;
                    for field in &mut object.fields {
                        let is_target_enum = field
                            .type_
                            .as_ref()
                            .and_then(|field_type| field_type.unresolved_name.as_deref())
                            .is_some_and(|name| type_name_matches(name, enum_name));
                        if is_target_enum && field.default_string.as_deref() == Some(old_name) {
                            field.default_string = Some(new_name.to_string());
                            changed = true;
                        }
                    }
                    changed.then(|| name.clone())
                }
                _ => None,
            })
            .collect();
        self.touched.extend(changed_defaults);
        self.touched.insert(enum_name.to_string());
        Ok(())
    }

    fn enum_value_references(&self, enum_name: &str, value_name: &str) -> Vec<String> {
        self.decls
            .iter()
            .filter_map(|(name, payload)| match payload {
                DeclPayload::Object(object)
                    if object.fields.iter().any(|field| {
                        field
                            .type_
                            .as_ref()
                            .and_then(|field_type| field_type.unresolved_name.as_deref())
                            .is_some_and(|name| type_name_matches(name, enum_name))
                            && field.default_string.as_deref() == Some(value_name)
                    }) =>
                {
                    Some(name.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn validate_removed_references(&self) -> Result<(), MutationError> {
        for name in &self.removed {
            let mut references: Vec<String> = self
                .decls
                .iter()
                .filter(|(_, payload)| payload_references(payload, name))
                .map(|(declaration_name, _)| declaration_name.clone())
                .collect();
            if self
                .meta
                .root_type
                .as_deref()
                .is_some_and(|root| type_name_matches(root, name))
            {
                references.push("root_type".to_string());
            }
            if !references.is_empty() {
                return Err(MutationError::InvalidOperation(format!(
                    "cannot remove declaration '{name}': referenced by {}",
                    references.join(", ")
                )));
            }
        }
        for (enum_name, value_name) in &self.removed_enum_values {
            let references = self.enum_value_references(enum_name, value_name);
            if !references.is_empty() {
                return Err(MutationError::InvalidOperation(format!(
                    "cannot remove enum value '{enum_name}.{value_name}': referenced as a default by {}",
                    references.join(", ")
                )));
            }
        }
        Ok(())
    }

    // ── Meta / imports ─────────────────────────────────────────────────────────

    fn update_import(
        &mut self,
        import_path: &str,
        resolved_commit: &str,
        remove: bool,
    ) -> Result<(), MutationError> {
        self.meta
            .include_commits
            .resize(self.meta.includes.len(), String::new());
        if remove {
            if let Some(index) = self
                .meta
                .includes
                .iter()
                .position(|include| include == import_path)
            {
                self.meta.includes.remove(index);
                self.meta.include_commits.remove(index);
            }
        } else if let Some(index) = self
            .meta
            .includes
            .iter()
            .position(|include| include == import_path)
        {
            self.meta.include_commits[index] = resolved_commit.to_string();
        } else {
            self.meta.includes.push(import_path.to_string());
            self.meta.include_commits.push(resolved_commit.to_string());
        }
        self.meta_changed = true;
        Ok(())
    }
}

fn payload_matches_kind(payload: &DeclPayload, expected: ExpectedDeclKind) -> bool {
    match (payload, expected) {
        (DeclPayload::Object(object), ExpectedDeclKind::Table) => !object.is_struct,
        (DeclPayload::Enum(declaration), ExpectedDeclKind::Enum) => !declaration.is_union,
        (DeclPayload::Enum(declaration), ExpectedDeclKind::Union) => declaration.is_union,
        _ => false,
    }
}

fn type_name_matches(type_name: &str, target: &str) -> bool {
    type_name == target
        || type_name
            .rsplit('.')
            .next()
            .is_some_and(|simple| simple == target)
}

fn rename_type_name(type_name: &str, old_name: &str, new_name: &str) -> Option<String> {
    if !type_name_matches(type_name, old_name) {
        return None;
    }
    match type_name.rfind('.') {
        Some(separator) => Some(format!("{}{new_name}", &type_name[..=separator])),
        None => Some(new_name.to_string()),
    }
}

fn payload_references(payload: &DeclPayload, target: &str) -> bool {
    match payload {
        DeclPayload::Object(object) => object.fields.iter().any(|field| {
            field
                .type_
                .as_ref()
                .and_then(|field_type| field_type.unresolved_name.as_deref())
                .is_some_and(|name| type_name_matches(name, target))
        }),
        DeclPayload::Enum(declaration) if declaration.is_union => {
            declaration.values.iter().any(|value| {
                value
                    .union_type
                    .as_ref()
                    .and_then(|member_type| member_type.unresolved_name.as_deref())
                    .is_some_and(|name| type_name_matches(name, target))
            })
        }
        DeclPayload::Service(service) => service.calls.iter().any(|call| {
            call.request
                .as_ref()
                .and_then(|request| request.name.as_deref())
                .is_some_and(|name| type_name_matches(name, target))
                || call
                    .response
                    .as_ref()
                    .and_then(|response| response.name.as_deref())
                    .is_some_and(|name| type_name_matches(name, target))
        }),
        DeclPayload::Enum(_) => false,
    }
}

fn rewrite_payload_references(payload: &mut DeclPayload, old_name: &str, new_name: &str) -> bool {
    match payload {
        DeclPayload::Object(object) => {
            let mut changed = false;
            for field in &mut object.fields {
                let Some(field_type) = &mut field.type_ else {
                    continue;
                };
                let Some(type_name) = &field_type.unresolved_name else {
                    continue;
                };
                if let Some(renamed) = rename_type_name(type_name, old_name, new_name) {
                    field_type.unresolved_name = Some(renamed);
                    changed = true;
                }
            }
            changed
        }
        DeclPayload::Enum(declaration) if declaration.is_union => {
            let mut changed = false;
            for value in &mut declaration.values {
                let Some(member_type) = &mut value.union_type else {
                    continue;
                };
                let Some(type_name) = &member_type.unresolved_name else {
                    continue;
                };
                if let Some(renamed) = rename_type_name(type_name, old_name, new_name) {
                    member_type.unresolved_name = Some(renamed);
                    if value.name.as_deref() == Some(old_name) {
                        value.name = Some(new_name.to_string());
                    }
                    changed = true;
                }
            }
            changed
        }
        DeclPayload::Service(service) => {
            let mut changed = false;
            for call in &mut service.calls {
                for object in [&mut call.request, &mut call.response]
                    .into_iter()
                    .flatten()
                {
                    let Some(name) = &object.name else {
                        continue;
                    };
                    if let Some(renamed) = rename_type_name(name, old_name, new_name) {
                        object.name = Some(renamed);
                        changed = true;
                    }
                }
            }
            changed
        }
        DeclPayload::Enum(_) => false,
    }
}

/// Set a value-less system attribute (e.g. `deprecated`) on a field if absent.
fn set_flag_attribute(field: &mut Field, key: &str) {
    let attrs = field.attributes.get_or_insert_with(Attributes::default);
    if !attrs.has(key) {
        attrs.entries.push(KeyValue {
            key: Some(key.to_string()),
            value: None,
        });
    }
}

/// Build a `Type` from a canonical `.fbs` type spec string.
///
/// Handles scalars, user-defined types, and vectors `[elem]`. (Fixed arrays
/// `[elem:N]` are not produced by mutations; vectors cover the common case.)
fn parse_type_spec(spec: &str) -> Type {
    let spec = spec.trim();
    if let Some(inner) = spec.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let elem = parse_type_spec(inner);
        return Type {
            base_type: Some(BaseType::BASE_TYPE_VECTOR),
            base_size: Some(4),
            element_type: elem.base_type,
            element_size: elem.base_size,
            unresolved_name: elem.unresolved_name,
            ..Default::default()
        };
    }
    match lookup_scalar(spec) {
        Some(bt) => Type {
            base_type: Some(bt),
            base_size: bt.byte_size(),
            ..Default::default()
        },
        None => Type {
            base_type: Some(BaseType::BASE_TYPE_TABLE),
            base_size: Some(4),
            unresolved_name: Some(spec.to_string()),
            ..Default::default()
        },
    }
}

/// Reverse of the canonical scalar vocabulary (mirrors the parser's
/// `lookup_base_type`).
fn lookup_scalar(name: &str) -> Option<BaseType> {
    Some(match name {
        "bool" => BaseType::BASE_TYPE_BOOL,
        "byte" | "int8" => BaseType::BASE_TYPE_BYTE,
        "ubyte" | "uint8" => BaseType::BASE_TYPE_U_BYTE,
        "short" | "int16" => BaseType::BASE_TYPE_SHORT,
        "ushort" | "uint16" => BaseType::BASE_TYPE_U_SHORT,
        "int" | "int32" => BaseType::BASE_TYPE_INT,
        "uint" | "uint32" => BaseType::BASE_TYPE_U_INT,
        "long" | "int64" => BaseType::BASE_TYPE_LONG,
        "ulong" | "uint64" => BaseType::BASE_TYPE_U_LONG,
        "float" | "float32" => BaseType::BASE_TYPE_FLOAT,
        "double" | "float64" => BaseType::BASE_TYPE_DOUBLE,
        "string" => BaseType::BASE_TYPE_STRING,
        _ => return None,
    })
}
