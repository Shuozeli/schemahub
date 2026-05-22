use crate::ast::{EnumBlob, MessageBlob, ServiceBlob};

/// Reconstruct a proto3 message fragment from a MessageBlob.
pub fn print_message(msg: &MessageBlob) -> String {
    let mut out = String::new();

    if !msg.doc_comment.is_empty() {
        for line in msg.doc_comment.lines() {
            out.push_str("// ");
            out.push_str(line);
            out.push('\n');
        }
    }

    out.push_str("message ");
    out.push_str(&msg.name);
    out.push_str(" {\n");

    let mut i = 0;
    while i < msg.fields.len() {
        let field = &msg.fields[i];

        if !field.map_key_type.is_empty() {
            // map<K, V> field
            if !field.doc_comment.is_empty() {
                for line in field.doc_comment.lines() {
                    out.push_str("  // ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out.push_str("  map<");
            out.push_str(&field.map_key_type);
            out.push_str(", ");
            out.push_str(&field.field_type);
            out.push_str("> ");
            out.push_str(&field.name);
            out.push_str(" = ");
            out.push_str(&field.number.to_string());
            out.push_str(";\n");
            i += 1;
        } else if !field.oneof_name.is_empty() {
            // Start of a oneof group — collect all consecutive fields with the same oneof name
            let oneof_name = field.oneof_name.clone();
            out.push_str("  oneof ");
            out.push_str(&oneof_name);
            out.push_str(" {\n");
            while i < msg.fields.len() && msg.fields[i].oneof_name == oneof_name {
                let f = &msg.fields[i];
                if !f.doc_comment.is_empty() {
                    for line in f.doc_comment.lines() {
                        out.push_str("    // ");
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                out.push_str("    ");
                out.push_str(&f.field_type);
                out.push(' ');
                out.push_str(&f.name);
                out.push_str(" = ");
                out.push_str(&f.number.to_string());
                out.push_str(";\n");
                i += 1;
            }
            out.push_str("  }\n");
        } else {
            // Regular (or repeated) field
            if !field.doc_comment.is_empty() {
                for line in field.doc_comment.lines() {
                    out.push_str("  // ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out.push_str("  ");
            if field.repeated {
                out.push_str("repeated ");
            }
            out.push_str(&field.field_type);
            out.push(' ');
            out.push_str(&field.name);
            out.push_str(" = ");
            out.push_str(&field.number.to_string());
            out.push_str(";\n");
            i += 1;
        }
    }

    for range in &msg.reserved_numbers {
        out.push_str("  reserved ");
        if range.start == range.end {
            out.push_str(&range.start.to_string());
        } else {
            out.push_str(&range.start.to_string());
            out.push_str(" to ");
            out.push_str(&range.end.to_string());
        }
        out.push_str(";\n");
    }

    for name in &msg.reserved_names {
        out.push_str("  reserved \"");
        out.push_str(name);
        out.push_str("\";\n");
    }

    out.push_str("}\n");
    out
}

/// Reconstruct a proto3 enum fragment from an EnumBlob.
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
    out.push_str(" {\n");

    for value in &e.values {
        if !value.doc_comment.is_empty() {
            for line in value.doc_comment.lines() {
                out.push_str("  // ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("  ");
        out.push_str(&value.name);
        out.push_str(" = ");
        out.push_str(&value.number.to_string());
        out.push_str(";\n");
    }

    out.push_str("}\n");
    out
}

/// Reconstruct a proto3 service fragment from a ServiceBlob.
pub fn print_service(s: &ServiceBlob) -> String {
    let mut out = String::new();

    if !s.doc_comment.is_empty() {
        for line in s.doc_comment.lines() {
            out.push_str("// ");
            out.push_str(line);
            out.push('\n');
        }
    }

    out.push_str("service ");
    out.push_str(&s.name);
    out.push_str(" {\n");

    for rpc in &s.rpcs {
        if !rpc.doc_comment.is_empty() {
            for line in rpc.doc_comment.lines() {
                out.push_str("  // ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str("  rpc ");
        out.push_str(&rpc.name);
        out.push('(');
        if rpc.client_streaming { out.push_str("stream "); }
        out.push_str(&rpc.request_type);
        out.push(')');
        out.push_str(" returns (");
        if rpc.server_streaming { out.push_str("stream "); }
        out.push_str(&rpc.response_type);
        out.push_str(");\n");
    }

    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{EnumValueDef, FieldDef, RpcDef};

    #[test]
    fn print_message_basic() {
        let msg = MessageBlob {
            blob_version: 1,
            name: "Foo".into(),
            fields: vec![
                FieldDef {
                    name: "bar".into(),
                    field_type: "string".into(),
                    number: 1,
                    repeated: false,
                    doc_comment: String::new(),
                    ..Default::default()
                },
            ],
            reserved_numbers: vec![],
            reserved_names: vec![],
            doc_comment: String::new(),
        };
        let text = print_message(&msg);
        assert!(text.contains("message Foo"));
        assert!(text.contains("string bar = 1;"));
    }

    #[test]
    fn print_enum_basic() {
        let e = EnumBlob {
            blob_version: 1,
            name: "Status".into(),
            values: vec![
                EnumValueDef { name: "UNKNOWN".into(), number: 0, doc_comment: String::new() },
            ],
            doc_comment: String::new(),
        };
        let text = print_enum(&e);
        assert!(text.contains("enum Status"));
        assert!(text.contains("UNKNOWN = 0;"));
    }

    #[test]
    fn print_message_with_oneof() {
        let msg = MessageBlob {
            blob_version: 1,
            name: "Event".into(),
            fields: vec![
                FieldDef { name: "id".into(), field_type: "string".into(), number: 1, ..Default::default() },
                FieldDef { name: "text".into(), field_type: "string".into(), number: 2, oneof_name: "payload".into(), ..Default::default() },
                FieldDef { name: "data".into(), field_type: "bytes".into(), number: 3, oneof_name: "payload".into(), ..Default::default() },
                FieldDef { name: "done".into(), field_type: "bool".into(), number: 4, ..Default::default() },
            ],
            reserved_numbers: vec![],
            reserved_names: vec![],
            doc_comment: String::new(),
        };
        let text = print_message(&msg);
        assert!(text.contains("oneof payload {"), "missing oneof block");
        assert!(text.contains("    string text = 2;"), "oneof field indented 4 spaces");
        assert!(text.contains("    bytes data = 3;"));
        assert!(text.contains("  string id = 1;"), "regular field at 2 spaces");
        assert!(text.contains("  bool done = 4;"));
    }

    #[test]
    fn print_message_with_map() {
        let msg = MessageBlob {
            blob_version: 1,
            name: "Config".into(),
            fields: vec![
                FieldDef {
                    name: "labels".into(),
                    field_type: "string".into(),
                    map_key_type: "string".into(),
                    number: 1,
                    ..Default::default()
                },
            ],
            reserved_numbers: vec![],
            reserved_names: vec![],
            doc_comment: String::new(),
        };
        let text = print_message(&msg);
        assert!(text.contains("map<string, string> labels = 1;"));
    }

    #[test]
    fn print_service_basic() {
        let svc = ServiceBlob {
            blob_version: 1,
            name: "FooService".into(),
            rpcs: vec![
                RpcDef {
                    name: "DoThing".into(),
                    request_type: "Request".into(),
                    response_type: "Response".into(),
                    client_streaming: false,
                    server_streaming: false,
                    doc_comment: String::new(),
                },
            ],
            doc_comment: String::new(),
        };
        let text = print_service(&svc);
        assert!(text.contains("service FooService"));
        assert!(text.contains("rpc DoThing(Request) returns (Response);"));
    }
}
