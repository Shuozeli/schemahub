use crate::ast::{EnumBlob, FileMetadataBlob, StructBlob, TableBlob, UnionBlob};

/// Print the file metadata section (namespace, includes, root_type, file_identifier).
pub fn print_metadata(meta: &FileMetadataBlob) -> String {
    let mut out = String::new();
    if !meta.namespace.is_empty() {
        out.push_str(&format!("namespace {};\n", meta.namespace));
    }
    for imp in &meta.imports {
        out.push_str(&format!("include \"{imp}\";\n"));
    }
    if !meta.root_type.is_empty() {
        out.push_str(&format!("root_type {};\n", meta.root_type));
    }
    for fi in &meta.file_identifier {
        out.push_str(&format!("file_identifier \"{fi}\";\n"));
    }
    out
}

/// Reconstruct a table definition from a TableBlob.
pub fn print_table(t: &TableBlob) -> String {
    let mut out = String::new();
    if !t.doc_comment.is_empty() {
        for line in t.doc_comment.lines() {
            out.push_str("// ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("table ");
    out.push_str(&t.name);
    out.push_str(" {\n");
    for field in &t.fields {
        if !field.doc_comment.is_empty() {
            for line in field.doc_comment.lines() {
                out.push_str("  // ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("  ");
        out.push_str(&field.name);
        out.push_str(": ");
        out.push_str(&field.field_type);
        if !field.default_value.is_empty() {
            out.push_str(" = ");
            out.push_str(&field.default_value);
        }
        if field.deprecated {
            out.push_str(" (deprecated)");
        }
        out.push_str(";\n");
    }
    out.push_str("}\n");
    out
}

/// Reconstruct a struct definition from a StructBlob.
pub fn print_struct(s: &StructBlob) -> String {
    let mut out = String::new();
    if !s.doc_comment.is_empty() {
        for line in s.doc_comment.lines() {
            out.push_str("// ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("struct ");
    out.push_str(&s.name);
    out.push_str(" {\n");
    for field in &s.fields {
        if !field.doc_comment.is_empty() {
            for line in field.doc_comment.lines() {
                out.push_str("  // ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("  ");
        out.push_str(&field.name);
        out.push_str(": ");
        out.push_str(&field.field_type);
        out.push_str(";\n");
    }
    out.push_str("}\n");
    out
}

/// Reconstruct an enum definition from an EnumBlob.
pub fn print_enum(e: &EnumBlob) -> String {
    let mut out = String::new();
    if !e.doc_comment.is_empty() {
        for line in e.doc_comment.lines() {
            out.push_str("// ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("enum ");
    out.push_str(&e.name);
    out.push_str(" : ");
    out.push_str(&e.base_type);
    out.push_str(" {\n");
    let len = e.values.len();
    for (i, val) in e.values.iter().enumerate() {
        if !val.doc_comment.is_empty() {
            for line in val.doc_comment.lines() {
                out.push_str("  // ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("  ");
        out.push_str(&val.name);
        out.push_str(" = ");
        out.push_str(&val.value.to_string());
        if i + 1 < len {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

/// Reconstruct a union definition from a UnionBlob.
pub fn print_union(u: &UnionBlob) -> String {
    let mut out = String::new();
    if !u.doc_comment.is_empty() {
        for line in u.doc_comment.lines() {
            out.push_str("// ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("union ");
    out.push_str(&u.name);
    out.push_str(" {\n");
    for member in &u.members {
        out.push_str("  ");
        out.push_str(member);
        out.push_str(",\n");
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{EnumValueDef, FieldDef, StructFieldDef};

    #[test]
    fn print_table_basic() {
        let t = TableBlob {
            name: "Order".into(),
            fields: vec![FieldDef {
                name: "id".into(),
                field_type: "string".into(),
                default_value: String::new(),
                deprecated: false,
                slot_index: 0,
                doc_comment: String::new(),
            }],
            doc_comment: String::new(),
        };
        let text = print_table(&t);
        assert!(text.contains("table Order"), "got: {text}");
        assert!(text.contains("id: string;"), "got: {text}");
    }

    #[test]
    fn print_table_with_default_and_deprecated() {
        let t = TableBlob {
            name: "T".into(),
            fields: vec![
                FieldDef {
                    name: "status".into(),
                    field_type: "int32".into(),
                    default_value: "0".into(),
                    deprecated: false,
                    slot_index: 0,
                    doc_comment: String::new(),
                },
                FieldDef {
                    name: "old".into(),
                    field_type: "string".into(),
                    default_value: String::new(),
                    deprecated: true,
                    slot_index: 1,
                    doc_comment: String::new(),
                },
            ],
            doc_comment: String::new(),
        };
        let text = print_table(&t);
        assert!(text.contains("status: int32 = 0;"), "got: {text}");
        assert!(text.contains("old: string (deprecated);"), "got: {text}");
    }

    #[test]
    fn print_struct_basic() {
        let s = StructBlob {
            name: "Vec3".into(),
            fields: vec![StructFieldDef {
                name: "x".into(),
                field_type: "float".into(),
                doc_comment: String::new(),
            }],
            doc_comment: String::new(),
        };
        let text = print_struct(&s);
        assert!(text.contains("struct Vec3"), "got: {text}");
        assert!(text.contains("x: float;"), "got: {text}");
    }

    #[test]
    fn print_enum_basic() {
        let e = EnumBlob {
            name: "Color".into(),
            base_type: "byte".into(),
            values: vec![
                EnumValueDef { name: "Red".into(), value: 0, doc_comment: String::new() },
                EnumValueDef { name: "Green".into(), value: 1, doc_comment: String::new() },
            ],
            doc_comment: String::new(),
        };
        let text = print_enum(&e);
        assert!(text.contains("enum Color : byte"), "got: {text}");
        assert!(text.contains("Red = 0"), "got: {text}");
        assert!(text.contains("Green = 1"), "got: {text}");
    }

    #[test]
    fn print_union_basic() {
        let u = UnionBlob {
            name: "Shape".into(),
            members: vec!["Circle".into(), "Square".into()],
            doc_comment: String::new(),
        };
        let text = print_union(&u);
        assert!(text.contains("union Shape"), "got: {text}");
        assert!(text.contains("Circle,"), "got: {text}");
        assert!(text.contains("Square,"), "got: {text}");
    }
}
