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
use schemahub_types::{MetaBlob, Mutation, MutationEffect, MutationError, SchemaObjects};

use crate::blob::{decode_decl, decode_meta, encode_decl, encode_meta, DeclPayload};

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
    /// Rename a union.
    RenameUnion { old_name: String, new_name: String },
    /// Delete a union.
    DeleteUnion { name: String },

    /// Add or remove an `include "..."` in the file metadata.
    UpdateImport { import_path: String, remove: bool },
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
    let mut state = WorkingState::load(schema)?;
    state.apply(&parsed)?;
    Ok(state.into_effect())
}

/// Apply an ordered batch of mutations; only the final state matters.
pub fn apply_mutations(
    schema: &SchemaObjects,
    ops: &[Mutation],
) -> Result<MutationEffect, MutationError> {
    let mut state = WorkingState::load(schema)?;
    for op in ops {
        let parsed = FbsOp::decode(&op.operation)?;
        state.apply(&parsed)?;
    }
    Ok(state.into_effect())
}

/// Mutable working copy of a schema's declarations plus change tracking.
struct WorkingState {
    /// Live, name-keyed decl payloads (mutated in place).
    decls: std::collections::BTreeMap<String, DeclPayload>,
    meta: MetaBlob,
    meta_changed: bool,
    /// Names that have been touched (upserted).
    touched: std::collections::BTreeSet<String>,
    /// Names removed.
    removed: std::collections::BTreeSet<String>,
}

impl WorkingState {
    fn load(schema: &SchemaObjects) -> Result<Self, MutationError> {
        let mut decls = std::collections::BTreeMap::new();
        for (name, blob) in &schema.decls {
            let payload =
                decode_decl(blob).map_err(|e| MutationError::MalformedBlob(e.to_string()))?;
            decls.insert(name.clone(), payload);
        }
        Ok(Self {
            decls,
            meta: schema.meta.clone(),
            meta_changed: false,
            touched: Default::default(),
            removed: Default::default(),
        })
    }

    fn into_effect(self) -> MutationEffect {
        let mut upserts = Vec::new();
        for name in &self.touched {
            if let Some(payload) = self.decls.get(name) {
                upserts.push((name.clone(), encode_decl(payload.clone())));
            }
        }
        MutationEffect {
            meta: if self.meta_changed {
                Some(self.meta)
            } else {
                None
            },
            upserts,
            removes: self.removed.into_iter().collect(),
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
            FbsOp::RenameTable { old_name, new_name } => self.rename_decl(old_name, new_name, false),
            FbsOp::DeleteTable { name } => self.delete_decl(name),

            FbsOp::CreateEnum {
                name,
                underlying_type,
                doc_comment,
            } => self.create_enum(name, underlying_type, doc_comment, false),
            FbsOp::RenameEnum { old_name, new_name } => self.rename_decl(old_name, new_name, true),
            FbsOp::DeleteEnum { name } => self.delete_decl(name),
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
            FbsOp::RenameUnion { old_name, new_name } => self.rename_decl(old_name, new_name, true),
            FbsOp::DeleteUnion { name } => self.delete_decl(name),

            FbsOp::UpdateImport {
                import_path,
                remove,
            } => self.update_import(import_path, *remove),
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
        self.touched.insert(name.to_string());
        Ok(())
    }

    fn rename_decl(
        &mut self,
        old_name: &str,
        new_name: &str,
        expect_enum: bool,
    ) -> Result<(), MutationError> {
        let mut payload = self
            .decls
            .remove(old_name)
            .ok_or_else(|| MutationError::DeclarationNotFound(old_name.to_string()))?;
        // Kind sanity check.
        match (&payload, expect_enum) {
            (DeclPayload::Enum(_), true) | (DeclPayload::Object(_), false) => {}
            _ => {
                // Restore and reject.
                self.decls.insert(old_name.to_string(), payload);
                return Err(MutationError::InvalidOperation(format!(
                    "declaration '{old_name}' kind does not match rename target"
                )));
            }
        }
        if self.decls.contains_key(new_name) {
            self.decls.insert(old_name.to_string(), payload);
            return Err(MutationError::InvalidOperation(format!(
                "cannot rename '{old_name}' to '{new_name}': target already exists"
            )));
        }
        match &mut payload {
            DeclPayload::Object(o) => o.name = Some(new_name.to_string()),
            DeclPayload::Enum(e) => e.name = Some(new_name.to_string()),
            DeclPayload::Service(s) => s.name = Some(new_name.to_string()),
        }
        self.decls.insert(new_name.to_string(), payload);
        self.removed.insert(old_name.to_string());
        self.touched.remove(old_name);
        self.touched.insert(new_name.to_string());
        Ok(())
    }

    fn delete_decl(&mut self, name: &str) -> Result<(), MutationError> {
        if self.decls.remove(name).is_none() {
            return Err(MutationError::DeclarationNotFound(name.to_string()));
        }
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
        let en = self.enum_mut(enum_name)?;
        if en
            .values
            .iter()
            .any(|v| v.name.as_deref() == Some(value_name))
        {
            return Err(MutationError::InvalidOperation(format!(
                "enum value '{value_name}' already exists in '{enum_name}'"
            )));
        }
        en.values.push(EnumVal {
            name: Some(value_name.to_string()),
            value: Some(value),
            ..Default::default()
        });
        self.touched.insert(enum_name.to_string());
        Ok(())
    }

    fn remove_enum_value(
        &mut self,
        enum_name: &str,
        value_name: &str,
    ) -> Result<(), MutationError> {
        let en = self.enum_mut(enum_name)?;
        let before = en.values.len();
        en.values.retain(|v| v.name.as_deref() != Some(value_name));
        if en.values.len() == before {
            return Err(MutationError::FieldNotFound {
                declaration: enum_name.to_string(),
                field: value_name.to_string(),
            });
        }
        self.touched.insert(enum_name.to_string());
        Ok(())
    }

    fn rename_enum_value(
        &mut self,
        enum_name: &str,
        old_name: &str,
        new_name: &str,
    ) -> Result<(), MutationError> {
        let en = self.enum_mut(enum_name)?;
        let val = en
            .values
            .iter_mut()
            .find(|v| v.name.as_deref() == Some(old_name))
            .ok_or_else(|| MutationError::FieldNotFound {
                declaration: enum_name.to_string(),
                field: old_name.to_string(),
            })?;
        val.name = Some(new_name.to_string());
        self.touched.insert(enum_name.to_string());
        Ok(())
    }

    // ── Meta / imports ─────────────────────────────────────────────────────────

    fn update_import(&mut self, import_path: &str, remove: bool) -> Result<(), MutationError> {
        let mut meta =
            decode_meta(&self.meta).map_err(|e| MutationError::MalformedBlob(e.to_string()))?;
        if remove {
            meta.includes.retain(|i| i != import_path);
        } else if !meta.includes.iter().any(|i| i == import_path) {
            meta.includes.push(import_path.to_string());
        }
        self.meta = encode_meta(meta);
        self.meta_changed = true;
        Ok(())
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
