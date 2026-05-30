//! Headline test (crate-structure.md §6): `parse → split → reassemble → print
//! → parse` yields a semantically equivalent `FileDescriptorProto`.
//!
//! These exercise exactly the features the v1 hand-rolled AST could not
//! represent: nested types, `oneof`, proto3 `optional`, repeated fields, enums,
//! a service with streaming RPCs, field options, and reserved ranges.

use std::collections::BTreeMap;

use protoc_rs_schema::{
    DescriptorProto, EnumDescriptorProto, FileDescriptorProto,
};
use schemahub_types::{Compiler, SchemaObjects};

use schemahub_compiler_protobuf::ProtobufCompiler;

/// Parse source, reassemble into a `SchemaObjects`, and return the printed text.
fn parse_print(source: &str) -> String {
    // Arrange
    let compiler = ProtobufCompiler::new();

    // Act
    let parsed = compiler.parse(source).expect("parse should succeed");
    let mut decls = BTreeMap::new();
    for (name, blob) in parsed.decls {
        decls.insert(name, blob);
    }
    let objects = SchemaObjects {
        meta: parsed.meta,
        decls,
    };
    compiler.print(&objects).expect("print should succeed")
}

/// Parse to a `FileDescriptorProto` directly (the upstream parser).
fn parse_fd(source: &str) -> FileDescriptorProto {
    protoc_rs_parser::parse(source).expect("upstream parse should succeed")
}

/// Compare two files for semantic equivalence, ignoring `source_code_info` and
/// the parser-only `source_span` fields (which printing legitimately changes,
/// since line/column positions shift).
fn assert_semantically_equal(a: &FileDescriptorProto, b: &FileDescriptorProto) {
    let mut a = a.clone();
    let mut b = b.clone();
    strip_spans(&mut a);
    strip_spans(&mut b);
    assert_eq!(a, b, "files differ semantically");
}

fn strip_spans(file: &mut FileDescriptorProto) {
    file.source_code_info = None;
    for m in &mut file.message_type {
        strip_message_spans(m);
    }
    for e in &mut file.enum_type {
        strip_enum_spans(e);
    }
}

fn strip_message_spans(m: &mut DescriptorProto) {
    m.source_span = None;
    for f in &mut m.field {
        f.source_span = None;
    }
    for n in &mut m.nested_type {
        strip_message_spans(n);
    }
    for e in &mut m.enum_type {
        strip_enum_spans(e);
    }
}

fn strip_enum_spans(e: &mut EnumDescriptorProto) {
    e.source_span = None;
    for v in &mut e.value {
        v.source_span = None;
    }
}

#[test]
fn roundtrip_full_featured_schema() {
    // Arrange
    let source = r#"syntax = "proto3";

package my.pkg;

import "other.proto";

message User {
  string id = 1;
  repeated string tags = 2;
  optional int32 age = 3;
  oneof contact {
    string email = 4;
    string phone = 5;
  }
  map<string, int32> scores = 6;
  message Nested {
    bool flag = 1;
  }
  enum Inner {
    A = 0;
    B = 1;
  }
  reserved 7;
  reserved 8 to 10;
  reserved "foo", "bar";
}

enum Color {
  RED = 0;
  GREEN = 1;
}

service Svc {
  rpc Get(User) returns (User);
  rpc Stream(stream User) returns (stream User);
}
"#;

    // Act
    let printed = parse_print(source);
    let original_fd = parse_fd(source);
    let reparsed_fd = parse_fd(&printed);

    // Assert
    assert_semantically_equal(&original_fd, &reparsed_fd);
}

/// Top-level declaration names in `.proto` text order (column-0 decls only).
fn top_level_order(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        // Top-level decls are unindented; nested ones are indented.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        for kw in ["message ", "enum ", "service "] {
            if let Some(rest) = line.strip_prefix(kw) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.push(name);
                }
                break;
            }
        }
    }
    out
}

#[test]
fn roundtrip_preserves_cross_kind_declaration_order() {
    // Arrange: a service first, then a message, an enum, and another message —
    // a source order that differs from both alphabetical (Alpha, Color, Req,
    // Resp) and kind-grouped (messages, then enums, then services).
    let source = r#"syntax = "proto3";

package demo;

service Alpha {
  rpc Ping(Req) returns (Resp);
}

message Req {
  string a = 1;
}

enum Color {
  UNKNOWN = 0;
}

message Resp {
  string b = 1;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: declarations print in original source order across kinds.
    assert_eq!(
        top_level_order(&printed),
        vec!["Alpha", "Req", "Color", "Resp"],
        "cross-kind declaration order not preserved.\nprinted:\n{printed}"
    );
}

#[test]
fn roundtrip_preserves_nested_types() {
    // Arrange
    let source = r#"syntax = "proto3";

message Outer {
  message Inner {
    int32 x = 1;
  }
  Inner inner = 1;
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    let outer = &reparsed.message_type[0];
    assert_eq!(outer.name.as_deref(), Some("Outer"));
    assert_eq!(outer.nested_type.len(), 1);
    assert_eq!(outer.nested_type[0].name.as_deref(), Some("Inner"));
}

#[test]
fn roundtrip_preserves_proto3_optional() {
    // Arrange
    let source = "syntax = \"proto3\";\n\nmessage M {\n  optional int32 a = 1;\n}\n";

    // Act
    let printed = parse_print(source);

    // Assert
    assert!(
        printed.contains("optional int32 a = 1"),
        "expected proto3 optional; got:\n{printed}"
    );
}

#[test]
fn roundtrip_preserves_map_fields() {
    // Arrange
    let source = "syntax = \"proto3\";\n\nmessage M {\n  map<string, int32> m = 1;\n}\n";

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    assert!(printed.contains("map<string, int32> m = 1"), "got:\n{printed}");
    // Map entry remains a synthetic nested message.
    assert_eq!(reparsed.message_type[0].nested_type.len(), 1);
}

#[test]
fn roundtrip_preserves_streaming_rpcs() {
    // Arrange
    let source = r#"syntax = "proto3";

message Req {
  int32 a = 1;
}

service S {
  rpc Bidi(stream Req) returns (stream Req);
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    let method = &reparsed.service[0].method[0];
    assert_eq!(method.client_streaming, Some(true));
    assert_eq!(method.server_streaming, Some(true));
}

#[test]
fn roundtrip_preserves_reserved_ranges() {
    // Arrange
    let source = r#"syntax = "proto3";

message M {
  int32 a = 1;
  reserved 2;
  reserved 5 to 7;
  reserved "old_field";
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    let m = &reparsed.message_type[0];
    // 2..3 and 5..8 (end-exclusive).
    assert!(m
        .reserved_range
        .iter()
        .any(|r| r.start == Some(2) && r.end == Some(3)));
    assert!(m
        .reserved_range
        .iter()
        .any(|r| r.start == Some(5) && r.end == Some(8)));
    assert!(m.reserved_name.contains(&"old_field".to_string()));
}

#[test]
fn roundtrip_preserves_field_options() {
    // Arrange
    let source = r#"syntax = "proto3";

message M {
  int32 a = 1 [deprecated = true];
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert
    assert!(printed.contains("deprecated = true"), "got:\n{printed}");
}

#[test]
fn roundtrip_preserves_package_and_imports() {
    // Arrange
    let source = r#"syntax = "proto3";

package a.b.c;

import "x.proto";

message M {
  int32 a = 1;
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    assert_eq!(reparsed.package.as_deref(), Some("a.b.c"));
    assert!(reparsed.dependency.contains(&"x.proto".to_string()));
}

// ===========================================================================
// Advanced round-trip fidelity cases (gap coverage).
//
// Each asserts `parse → split → reassemble → print → parse` preserves a
// construct the base tests above don't exercise. Cases that surface a GENUINE
// printer/split bug keep the correct (currently-failing) expectation and are
// marked `#[ignore = "BUG: ..."]` so the green suite stays green while the bug
// is documented and re-checkable. See the test-suite report for repros.
// ===========================================================================

/// Round-trip a source and return the file printed by schemahub's own printer
/// reparsed through the upstream parser — the core fidelity comparison.
fn roundtrip_fd(source: &str) -> FileDescriptorProto {
    parse_fd(&parse_print(source))
}

// --- 1. proto2 syntax: labels + defaults --------------------------------------

#[test]
fn roundtrip_proto2_labels_and_defaults() {
    // Arrange
    let source = r#"syntax = "proto2";

enum E {
  E_UNSET = 0;
  E_ON = 1;
}

message M {
  required int32 a = 1;
  optional string s = 2 [default = "hi"];
  optional int32 n = 3 [default = 7];
  optional bool b = 4 [default = true];
  optional E e = 5 [default = E_ON];
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: labels survive...
    assert!(printed.contains("required int32 a"), "got:\n{printed}");
    assert!(printed.contains("optional string s"), "got:\n{printed}");
    // ...and the defaults survive (the actual bug: these are dropped).
    assert!(printed.contains("default = \"hi\""), "string default lost:\n{printed}");
    assert!(printed.contains("default = 7"), "int default lost:\n{printed}");
    assert!(printed.contains("default = true"), "bool default lost:\n{printed}");
    assert!(printed.contains("default = E_ON"), "enum default lost:\n{printed}");
    // And it should be a full semantic round-trip.
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

#[test]
fn roundtrip_proto2_required_label_survives() {
    // Arrange: `required` (unlike `optional`) IS emitted by the printer, so a
    // proto2 message built only from required fields round-trips cleanly.
    let source = r#"syntax = "proto2";

message M {
  required int32 a = 1;
  required string b = 2;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert
    assert!(printed.contains("required int32 a = 1"), "got:\n{printed}");
    assert!(printed.contains("required string b = 2"), "got:\n{printed}");
    assert_eq!(parse_fd(&printed).syntax.as_deref(), Some("proto2"));
}

#[test]
fn roundtrip_proto2_group() {
    // Arrange
    let source = r#"syntax = "proto2";

message M {
  optional group Result = 1 {
    optional int32 x = 1;
  }
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: the `group` keyword should survive (it does not — bug).
    assert!(printed.contains("group Result"), "group form lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 2. proto2 extensions -----------------------------------------------------

#[test]
fn roundtrip_proto2_extensions() {
    // Arrange
    let source = r#"syntax = "proto2";

message Foo {
  optional int32 a = 1;
  extensions 100 to 200;
}

extend Foo {
  optional int32 bar = 100;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: both the extensions range and the extend block should survive.
    assert!(printed.contains("extensions 100 to 200"), "extensions range lost:\n{printed}");
    assert!(printed.contains("extend Foo"), "extend block lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 3. editions --------------------------------------------------------------

#[test]
fn roundtrip_editions_2023_line_survives() {
    // Arrange
    let source = r#"edition = "2023";

message M {
  int32 a = 1;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: the `edition` line is preserved (not downgraded to `syntax`).
    assert!(printed.contains("edition = \"2023\""), "edition line lost:\n{printed}");
    assert!(!printed.contains("syntax ="), "edition downgraded to syntax:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 4. enum fidelity: reserved + allow_alias + aliases -----------------------

#[test]
fn roundtrip_enum_reserved_allow_alias_and_aliases() {
    // Arrange
    let source = r#"syntax = "proto3";

enum Status {
  option allow_alias = true;
  STATUS_UNSET = 0;
  STATUS_ACTIVE = 1;
  STATUS_RUNNING = 1;
  reserved 3, 5 to 7;
  reserved "OLD";
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: allow_alias, both aliased value names, and reserved survive.
    assert!(printed.contains("allow_alias = true"), "allow_alias lost:\n{printed}");
    assert!(printed.contains("STATUS_ACTIVE = 1"), "got:\n{printed}");
    assert!(printed.contains("STATUS_RUNNING = 1"), "alias value lost:\n{printed}");
    assert!(printed.contains("reserved \"OLD\""), "enum reserved name lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 5. options fidelity ------------------------------------------------------

#[test]
fn roundtrip_message_level_option() {
    // Arrange
    let source = r#"syntax = "proto3";

message M {
  option deprecated = true;
  int32 a = 1;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert
    assert!(printed.contains("option deprecated = true"), "message option lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

#[test]
fn roundtrip_enum_level_option() {
    // Arrange
    let source = r#"syntax = "proto3";

enum E {
  option deprecated = true;
  ZERO = 0;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert
    assert!(printed.contains("option deprecated = true"), "enum option lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

#[test]
fn roundtrip_field_deprecated_option_survives() {
    // Arrange: the `deprecated` field option IS printed (json_name is not — see
    // the ignored test below).
    let source = r#"syntax = "proto3";

message M {
  int32 a = 1 [deprecated = true];
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert
    assert!(printed.contains("deprecated = true"), "got:\n{printed}");
}

#[test]
fn roundtrip_field_json_name_option() {
    // Arrange
    let source = r#"syntax = "proto3";

message M {
  int32 a = 1 [json_name = "x", deprecated = true];
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: both options should survive; json_name is dropped (bug).
    assert!(printed.contains("deprecated = true"), "got:\n{printed}");
    assert!(printed.contains("json_name = \"x\""), "json_name option lost:\n{printed}");
}

// --- 6. deep + recursive nesting ----------------------------------------------

#[test]
fn roundtrip_three_level_deep_nesting() {
    // Arrange
    let source = r#"syntax = "proto3";

message L1 {
  message L2 {
    message L3 {
      int32 x = 1;
    }
    L3 l3 = 1;
  }
  L2 l2 = 1;
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert: the 3-level nesting chain survives intact.
    let l1 = &reparsed.message_type[0];
    let l2 = &l1.nested_type[0];
    let l3 = &l2.nested_type[0];
    assert_eq!(l1.name.as_deref(), Some("L1"));
    assert_eq!(l2.name.as_deref(), Some("L2"));
    assert_eq!(l3.name.as_deref(), Some("L3"));
    assert_semantically_equal(&parse_fd(source), &reparsed);
}

#[test]
fn roundtrip_self_referential_message() {
    // Arrange
    let source = r#"syntax = "proto3";

message Tree {
  repeated Tree children = 1;
  int32 value = 2;
}
"#;

    // Act / Assert
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

#[test]
fn roundtrip_mutually_referential_messages() {
    // Arrange
    let source = r#"syntax = "proto3";

message A {
  B b = 1;
}

message B {
  A a = 1;
}
"#;

    // Act / Assert
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 7. multiple oneofs + message-typed oneof members -------------------------

#[test]
fn roundtrip_multiple_oneofs_with_message_members() {
    // Arrange: two oneofs in one message; the second has a message-typed member.
    let source = r#"syntax = "proto3";

message Place {
  string city = 1;
}

message M {
  oneof first {
    string a = 1;
    int32 b = 2;
  }
  oneof second {
    Place place = 3;
    bool flag = 4;
  }
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert: oneof grouping + member order survive.
    let m = reparsed
        .message_type
        .iter()
        .find(|m| m.name.as_deref() == Some("M"))
        .expect("message M present");
    assert_eq!(m.oneof_decl.len(), 2, "expected two oneofs:\n{printed}");
    assert_eq!(m.oneof_decl[0].name.as_deref(), Some("first"));
    assert_eq!(m.oneof_decl[1].name.as_deref(), Some("second"));
    // Field order within the message is preserved (a, b, place, flag).
    let names: Vec<&str> = m.field.iter().filter_map(|f| f.name.as_deref()).collect();
    assert_eq!(names, vec!["a", "b", "place", "flag"], "got:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &reparsed);
}

// --- 8. map with message value ------------------------------------------------

#[test]
fn roundtrip_map_with_message_and_scalar_values() {
    // Arrange
    let source = r#"syntax = "proto3";

message Address {
  string city = 1;
}

message M {
  map<string, Address> by_id = 1;
  map<int32, int32> counts = 2;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: both print as `map<...>`, NOT as the synthetic entry message.
    assert!(printed.contains("map<string, Address> by_id = 1"), "got:\n{printed}");
    assert!(printed.contains("map<int32, int32> counts = 2"), "got:\n{printed}");
    assert!(!printed.contains("ByIdEntry"), "synthetic map entry leaked:\n{printed}");
    assert!(!printed.contains("CountsEntry"), "synthetic map entry leaked:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 9. reserved to max -------------------------------------------------------

#[test]
fn roundtrip_reserved_to_max() {
    // Arrange
    let source = r#"syntax = "proto3";

message M {
  int32 a = 1;
  reserved 100 to max;
}
"#;

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert: the printer materializes `max` as the numeric max field number
    // (536870911); this still reparses to the SAME reserved range, so the
    // round-trip is semantically faithful.
    assert!(printed.contains("reserved 100 to"), "reserved-to-max lost:\n{printed}");
    let m = &reparsed.message_type[0];
    assert!(
        m.reserved_range
            .iter()
            .any(|r| r.start == Some(100) && r.end == Some(536870912)),
        "reserved 100..max not preserved; ranges = {:?}",
        m.reserved_range
    );
    assert_semantically_equal(&parse_fd(source), &reparsed);
}

// --- 10. comments -------------------------------------------------------------

#[test]
fn roundtrip_leading_comments_survive() {
    // Arrange
    let source = r#"syntax = "proto3";

// leading comment on M
message M {
  // leading comment on field a
  int32 a = 1;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: leading comments on the message and the field are preserved.
    assert!(printed.contains("// leading comment on M"), "msg comment lost:\n{printed}");
    assert!(printed.contains("// leading comment on field a"), "field comment lost:\n{printed}");
}

#[test]
fn roundtrip_trailing_comment_survives() {
    // Arrange: a field with ONLY a trailing comment.
    let source = r#"syntax = "proto3";

message M {
  int32 a = 1; // trailing comment
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: the field round-trips AND the trailing comment is now preserved
    // (the printer emits `trailing_comments` from source_code_info as an inline
    // `// ...` suffix after the field).
    assert!(printed.contains("int32 a = 1"), "field lost:\n{printed}");
    assert!(
        printed.contains("// trailing comment"),
        "trailing comment dropped:\n{printed}"
    );
    assert_semantically_equal(&parse_fd(source), &parse_fd(&printed));
}

#[test]
fn roundtrip_detached_comment_survives() {
    // Arrange: a detached comment (separated by a blank line) plus a leading
    // comment immediately above the message.
    let source = r#"syntax = "proto3";

// detached comment

// leading comment on M
message M {
  int32 a = 1;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: BOTH the leading and the detached comment now survive (the printer
    // emits `leading_detached_comments` as a separate block before the decl).
    assert!(printed.contains("// leading comment on M"), "leading comment lost:\n{printed}");
    assert!(
        printed.contains("// detached comment"),
        "detached comment dropped:\n{printed}"
    );
}

// --- 11. well-known types -----------------------------------------------------

#[test]
fn roundtrip_well_known_type_qualified_names() {
    // Arrange
    let source = r#"syntax = "proto3";

import "google/protobuf/any.proto";
import "google/protobuf/duration.proto";
import "google/protobuf/timestamp.proto";

message M {
  google.protobuf.Any any = 1;
  google.protobuf.Duration dur = 2;
  google.protobuf.Timestamp ts = 3;
}
"#;

    // Act
    let printed = parse_print(source);

    // Assert: the QUALIFIED names survive (the historical truncation bug turned
    // these into bare `Any`/`Duration`/`Timestamp`).
    assert!(printed.contains("google.protobuf.Any any = 1"), "Any truncated:\n{printed}");
    assert!(printed.contains("google.protobuf.Duration dur = 2"), "Duration truncated:\n{printed}");
    assert!(printed.contains("google.protobuf.Timestamp ts = 3"), "Timestamp truncated:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}

// --- 12. edge cases -----------------------------------------------------------

#[test]
fn roundtrip_empty_message() {
    // Arrange
    let source = "syntax = \"proto3\";\n\nmessage Empty {\n}\n";

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    assert_eq!(reparsed.message_type[0].name.as_deref(), Some("Empty"));
    assert!(reparsed.message_type[0].field.is_empty());
    assert_semantically_equal(&parse_fd(source), &reparsed);
}

#[test]
fn roundtrip_enum_with_only_zero_value() {
    // Arrange
    let source = "syntax = \"proto3\";\n\nenum E {\n  ZERO = 0;\n}\n";

    // Act
    let printed = parse_print(source);
    let reparsed = parse_fd(&printed);

    // Assert
    let e = &reparsed.enum_type[0];
    assert_eq!(e.value.len(), 1);
    assert_eq!(e.value[0].name.as_deref(), Some("ZERO"));
    assert_eq!(e.value[0].number, Some(0));
    assert_semantically_equal(&parse_fd(source), &reparsed);
}

#[test]
fn roundtrip_unicode_in_comment() {
    // Arrange: raw UTF-8 (the parser rejects `\u`-style escapes, so we use
    // literal multibyte characters) in a leading comment.
    let source = "syntax = \"proto3\";\n\n// café 日本語 \u{1f600} emoji\nmessage M {\n  int32 a = 1;\n}\n";

    // Act
    let printed = parse_print(source);

    // Assert: the unicode comment survives byte-for-byte.
    assert!(printed.contains("// café 日本語 \u{1f600} emoji"), "unicode comment lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &parse_fd(&printed));
}

#[test]
fn roundtrip_unicode_in_string_default() {
    // Arrange
    let source = "syntax = \"proto2\";\n\nmessage M {\n  optional string s = 1 [default = \"héllo 日本語\"];\n}\n";

    // Act
    let printed = parse_print(source);

    // Assert: the unicode default should survive.
    assert!(printed.contains("default = \"héllo 日本語\""), "unicode default lost:\n{printed}");
    assert_semantically_equal(&parse_fd(source), &roundtrip_fd(source));
}
