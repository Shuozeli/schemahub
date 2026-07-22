//! End-to-end format round-trip suite driving the REAL in-process gRPC server.
//! Exercises FlatBuffers, OpenAPI, and complex Protobuf create→pull round-trips
//! plus exploration (ListDeclarations / GetDeclaration). Arrange-Act-Assert
//! throughout. Uses the shared `common` harness.

mod common;

use common::*;
use schemahub_api::schemahub_v1 as pb;

// ── helpers ────────────────────────────────────────────────────────────────

/// Top-level declaration names in the order they appear in `.fbs` text. Scans
/// for `table|struct|enum|union|rpc_service <Name>` lines.
fn ordered_fbs_decl_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in src.lines() {
        let t = line.trim_start();
        for kw in ["table ", "struct ", "enum ", "union ", "rpc_service "] {
            if let Some(rest) = t.strip_prefix(kw) {
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

/// Fetch one declaration's detail bytes as a UTF-8 string.
async fn get_declaration_detail(
    c: &mut pb::exploration_service_client::ExplorationServiceClient<tonic::transport::Channel>,
    project: &str,
    repo: &str,
    schema_path: &str,
    declaration_name: &str,
    at: pb::VersionRef,
) -> (pb::DeclSummary, Vec<u8>) {
    let r = c
        .get_declaration(pb::GetDeclarationRequest {
            project: project.into(),
            repo: repo.into(),
            schema_path: schema_path.into(),
            declaration_name: declaration_name.into(),
            at: Some(at),
        })
        .await
        .expect("get_declaration")
        .into_inner();
    (r.summary.expect("summary present"), r.detail)
}

/// List declarations with their kinds (i32 -> name) at a ref.
async fn list_decls_with_kinds(
    c: &mut pb::exploration_service_client::ExplorationServiceClient<tonic::transport::Channel>,
    project: &str,
    repo: &str,
    schema_path: &str,
    at: pb::VersionRef,
) -> Vec<(String, i32)> {
    c.list_declarations(pb::ListDeclarationsRequest {
        project: project.into(),
        repo: repo.into(),
        schema_path: schema_path.into(),
        at: Some(at),
        kind_filter: pb::DeclKind::Unspecified as i32,
    })
    .await
    .expect("list_declarations")
    .into_inner()
    .declarations
    .into_iter()
    .map(|d| (d.name, d.kind))
    .collect()
}

// ── fixtures ─────────────────────────────────────────────────────────────────

/// A `.fbs` declaring (in interleaved, non-alphabetical source order):
/// enum, union, struct, table, table, rpc_service. Alphabetical order would be
/// Color, GameService, Vec3, Weapon, Monster, AnyEntity — distinctly different.
const SAMPLE_FBS: &str = r#"namespace MyGame.Sample;

/// A color enum.
enum Color : byte {
  Red = 0,
  Green = 1,
  Blue = 2
}

union AnyEntity {
  Monster,
  Weapon
}

struct Vec3 {
  x: float;
  y: float;
  z: float;
}

table Monster {
  name: string (required);
  hp: short = 100;
  color: Color;
  old_field: int (deprecated);
}

table Weapon {
  damage: int;
}

rpc_service GameService {
  Lookup(Monster): Monster;
}

root_type Monster;
"#;

/// An OpenAPI 3.0.x document: info, two paths (GET with a query param, POST with
/// a requestBody + responses), and two component schemas where Order $refs User.
const SAMPLE_OPENAPI: &str = r#"openapi: "3.0.3"
info:
  title: "Orders API"
  version: "1.0.0"
paths:
  /users:
    get:
      operationId: listUsers
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
      responses:
        '200':
          description: Success
  /orders:
    post:
      operationId: createOrder
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Order'
      responses:
        '201':
          description: Created
components:
  schemas:
    User:
      type: object
      properties:
        id:
          type: string
        name:
          type: string
      required:
        - id
    Order:
      type: object
      properties:
        id:
          type: string
        owner:
          $ref: '#/components/schemas/User'
"#;

/// A proto3 file using: a map field, a nested message, a oneof, a `proto3
/// optional` field, an enum, a service with a server-streaming rpc, and a
/// `google.protobuf.Timestamp` field with the matching import.
const COMPLEX_PROTO: &str = r#"syntax = "proto3";
package shop.v1;

import "google/protobuf/timestamp.proto";

enum OrderStatus {
  ORDER_STATUS_UNSPECIFIED = 0;
  ORDER_STATUS_PAID = 1;
}

message Order {
  message LineItem {
    string sku = 1;
    int32 quantity = 2;
  }

  string id = 1;
  map<string, int32> tags = 2;
  LineItem first_item = 3;
  optional string coupon = 4;
  google.protobuf.Timestamp created_at = 5;
  OrderStatus status = 6;

  oneof payment {
    string card_token = 7;
    string invoice_id = 8;
  }
}

service OrderService {
  rpc WatchOrders(Order) returns (stream Order);
}
"#;

const RICH_INVENTORY_PROTO: &str = include_str!("../../../tests/integration/rich_inventory.proto");
const COMMERCE_CATALOG_FBS: &str = include_str!("../../../tests/integration/commerce_catalog.fbs");

// ── 1. FlatBuffers round-trip ──────────────────────────────────────────────────

#[tokio::test]
async fn flatbuffers_round_trip_preserves_decls_order_and_markers() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "game",
        "api",
        "main",
        "monster.fbs",
        pb::SchemaFormat::Flatbuffers,
        SAMPLE_FBS,
        "fbs-rt-1",
    )
    .await;

    // Act
    let pulled = pull_source(
        &mut c.explore,
        "game",
        "api",
        "monster.fbs",
        vref_branch("main"),
    )
    .await;

    // Assert: (a) every declaration survives.
    for needle in [
        "enum Color",
        "union AnyEntity",
        "struct Vec3",
        "table Monster",
        "table Weapon",
        "rpc_service GameService",
        "namespace MyGame.Sample",
    ] {
        assert!(pulled.contains(needle), "lost `{needle}`:\n{pulled}");
    }

    // Assert: (b) cross-kind declaration order matches SOURCE order (not alphabetical).
    let order = ordered_fbs_decl_names(&pulled);
    assert_eq!(
        order,
        vec![
            "Color",
            "AnyEntity",
            "Vec3",
            "Monster",
            "Weapon",
            "GameService"
        ],
        "fbs source order not preserved:\n{pulled}"
    );

    // Assert: (c) required/deprecated markers survive.
    assert!(
        pulled.contains("(required)"),
        "`required` marker lost:\n{pulled}"
    );
    assert!(
        pulled.contains("(deprecated)"),
        "`deprecated` marker lost:\n{pulled}"
    );
}

// ── 2. FlatBuffers exploration ──────────────────────────────────────────────────

#[tokio::test]
async fn flatbuffers_exploration_lists_decls_and_classifies_kinds() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "game",
        "api",
        "main",
        "monster.fbs",
        pb::SchemaFormat::Flatbuffers,
        SAMPLE_FBS,
        "fbs-explore-1",
    )
    .await;

    // Act
    let names = list_decl_names(
        &mut c.explore,
        "game",
        "api",
        "monster.fbs",
        vref_branch("main"),
    )
    .await;
    let kinds = list_decls_with_kinds(
        &mut c.explore,
        "game",
        "api",
        "monster.fbs",
        vref_branch("main"),
    )
    .await;
    let (table_summary, table_detail) = get_declaration_detail(
        &mut c.explore,
        "game",
        "api",
        "monster.fbs",
        "Monster",
        vref_branch("main"),
    )
    .await;

    // Assert: list_decl_names returns all decls.
    for n in [
        "Color",
        "AnyEntity",
        "Vec3",
        "Monster",
        "Weapon",
        "GameService",
    ] {
        assert!(
            names.contains(&n.to_string()),
            "missing decl `{n}`: {names:?}"
        );
    }

    // Assert: get_declaration on a table returns a non-empty detail.
    assert_eq!(table_summary.name, "Monster");
    assert!(
        !table_detail.is_empty(),
        "Monster detail should be non-empty"
    );

    // Assert: the reported DeclKinds for a table vs enum vs union differ appropriately.
    let kind_of = |name: &str| -> i32 {
        kinds
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("decl `{name}` not found in {kinds:?}"))
            .1
    };
    assert_eq!(
        kind_of("Monster"),
        pb::DeclKind::Table as i32,
        "Monster should be TABLE"
    );
    assert_eq!(
        kind_of("Color"),
        pb::DeclKind::FbsEnum as i32,
        "Color should be FBS_ENUM"
    );
    assert_eq!(
        kind_of("AnyEntity"),
        pb::DeclKind::Union as i32,
        "AnyEntity should be UNION"
    );
    assert_eq!(
        kind_of("Vec3"),
        pb::DeclKind::Struct as i32,
        "Vec3 should be STRUCT"
    );
    assert_eq!(
        kind_of("GameService"),
        pb::DeclKind::Service as i32,
        "GameService should be SERVICE"
    );
    // And they must actually differ.
    assert_ne!(kind_of("Monster"), kind_of("Color"));
    assert_ne!(kind_of("Color"), kind_of("AnyEntity"));
}

// ── 3. OpenAPI round-trip ──────────────────────────────────────────────────────

#[tokio::test]
async fn openapi_round_trip_preserves_paths_schemas_and_ref() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "shop",
        "api",
        "main",
        "api.yaml",
        pb::SchemaFormat::Openapi,
        SAMPLE_OPENAPI,
        "oa-rt-1",
    )
    .await;

    // Act
    let pulled = pull_source(
        &mut c.explore,
        "shop",
        "api",
        "api.yaml",
        vref_branch("main"),
    )
    .await;

    // Assert: both paths survive.
    assert!(pulled.contains("/users"), "path /users lost:\n{pulled}");
    assert!(pulled.contains("/orders"), "path /orders lost:\n{pulled}");
    // The operations survive.
    assert!(pulled.contains("listUsers"), "GET op lost:\n{pulled}");
    assert!(pulled.contains("createOrder"), "POST op lost:\n{pulled}");
    assert!(
        pulled.contains("requestBody"),
        "requestBody lost:\n{pulled}"
    );
    // Query parameter survives.
    assert!(pulled.contains("limit"), "query param lost:\n{pulled}");

    // Assert: both component schema names survive.
    assert!(pulled.contains("User"), "schema User lost:\n{pulled}");
    assert!(pulled.contains("Order"), "schema Order lost:\n{pulled}");

    // Assert: the cross-schema $ref is preserved (canonical form).
    assert!(
        pulled.contains("$ref: '#/components/schemas/User'"),
        "Order -> User $ref lost:\n{pulled}"
    );
    assert!(
        pulled.contains("$ref: '#/components/schemas/Order'"),
        "requestBody -> Order $ref lost:\n{pulled}"
    );
}

// ── 4. OpenAPI exploration ──────────────────────────────────────────────────────

#[tokio::test]
async fn openapi_exploration_lists_prefixed_keys_and_returns_schema_detail() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "shop",
        "api",
        "main",
        "api.yaml",
        pb::SchemaFormat::Openapi,
        SAMPLE_OPENAPI,
        "oa-explore-1",
    )
    .await;

    // Act
    let names = list_decl_names(
        &mut c.explore,
        "shop",
        "api",
        "api.yaml",
        vref_branch("main"),
    )
    .await;
    let (schema_summary, schema_detail) = get_declaration_detail(
        &mut c.explore,
        "shop",
        "api",
        "api.yaml",
        "schema:User",
        vref_branch("main"),
    )
    .await;

    // Assert: path:-prefixed and schema:-prefixed keys are present.
    assert!(
        names.contains(&"path:/users".to_string()),
        "missing path:/users: {names:?}"
    );
    assert!(
        names.contains(&"path:/orders".to_string()),
        "missing path:/orders: {names:?}"
    );
    assert!(
        names.contains(&"schema:User".to_string()),
        "missing schema:User: {names:?}"
    );
    assert!(
        names.contains(&"schema:Order".to_string()),
        "missing schema:Order: {names:?}"
    );

    // Assert: get_declaration on a schema:<Name> decl returns non-empty detail.
    assert_eq!(schema_summary.name, "schema:User");
    assert_eq!(
        schema_summary.kind,
        pb::DeclKind::ComponentSchema as i32,
        "schema:User should be COMPONENT_SCHEMA"
    );
    assert!(
        !schema_detail.is_empty(),
        "schema:User detail should be non-empty"
    );
}

// ── 5. Complex Protobuf round-trip ──────────────────────────────────────────────

#[tokio::test]
async fn complex_protobuf_round_trip_preserves_map_oneof_optional_nested_and_qualified_type() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "shop",
        "api",
        "main",
        "order.proto",
        pb::SchemaFormat::Protobuf,
        COMPLEX_PROTO,
        "pb-complex-1",
    )
    .await;

    // Act
    let pulled = pull_source(
        &mut c.explore,
        "shop",
        "api",
        "order.proto",
        vref_branch("main"),
    )
    .await;

    // Assert: the map field survives.
    assert!(
        pulled.contains("map<string, int32>"),
        "map field lost:\n{pulled}"
    );
    // Assert: the nested message survives.
    assert!(
        pulled.contains("message LineItem"),
        "nested message lost:\n{pulled}"
    );
    // Assert: the oneof survives.
    assert!(pulled.contains("oneof payment"), "oneof lost:\n{pulled}");
    assert!(pulled.contains("card_token"), "oneof field lost:\n{pulled}");
    // Assert: the proto3 `optional` field survives.
    assert!(
        pulled.contains("optional string coupon"),
        "proto3 optional lost:\n{pulled}"
    );
    // Assert: the enum survives.
    assert!(pulled.contains("enum OrderStatus"), "enum lost:\n{pulled}");
    // Assert: the streaming rpc survives.
    assert!(
        pulled.contains("returns (stream Order)"),
        "server-streaming rpc lost:\n{pulled}"
    );
    // Assert: the qualified type is preserved (NOT truncated to bare `Timestamp`).
    assert!(
        pulled.contains("google.protobuf.Timestamp created_at"),
        "qualified google.protobuf.Timestamp truncated or lost:\n{pulled}"
    );
    // And the import survives.
    assert!(
        pulled.contains("import \"google/protobuf/timestamp.proto\""),
        "timestamp import lost:\n{pulled}"
    );
}

// ── 6. Rich Protobuf fixture ──────────────────────────────────────────────────

#[tokio::test]
async fn rich_protobuf_fixture_preserves_business_schema_features_and_imports() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "inventory",
        "catalog",
        "main",
        "rich_inventory.proto",
        pb::SchemaFormat::Protobuf,
        RICH_INVENTORY_PROTO,
        "pb-rich-fixture-1",
    )
    .await;

    // Act
    let pulled = pull_source(
        &mut c.explore,
        "inventory",
        "catalog",
        "rich_inventory.proto",
        vref_branch("main"),
    )
    .await;
    let names = list_decl_names(
        &mut c.explore,
        "inventory",
        "catalog",
        "rich_inventory.proto",
        vref_branch("main"),
    )
    .await;
    let deps = c
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "inventory".into(),
            repo: "catalog".into(),
            schema_path: "rich_inventory.proto".into(),
            at: Some(vref_branch("main")),
            transitive: false,
        })
        .await
        .expect("list_dependencies")
        .into_inner()
        .dependencies;

    // Assert: top-level declarations are addressable.
    for n in [
        "Money",
        "Warehouse",
        "ProductStatus",
        "Product",
        "InventoryEvent",
        "InventoryService",
    ] {
        assert!(
            names.contains(&n.to_string()),
            "missing proto declaration `{n}`: {names:?}"
        );
    }

    // Assert: rich proto features survive canonical print.
    for needle in [
        "message Address",
        "message Dimensions",
        "map<string, string> labels",
        "map<string, string> attributes",
        "optional string gtin",
        "oneof purchasable_unit",
        "reserved 8",
        "reserved 12 to 15",
        "reserved \"legacy_sku\"",
        "google.protobuf.Timestamp created_at",
        "google.protobuf.Duration shelf_life",
        "returns (stream InventoryEvent)",
    ] {
        assert!(pulled.contains(needle), "lost `{needle}`:\n{pulled}");
    }

    // Assert: imports are tracked as dependencies.
    let dep_paths: Vec<String> = deps.into_iter().map(|d| d.import_path).collect();
    assert!(
        dep_paths.contains(&"google/protobuf/timestamp.proto".to_string()),
        "timestamp dependency missing: {dep_paths:?}"
    );
    assert!(
        dep_paths.contains(&"google/protobuf/duration.proto".to_string()),
        "duration dependency missing: {dep_paths:?}"
    );
}

// ── 7. Rich FlatBuffers fixture ───────────────────────────────────────────────

#[tokio::test]
async fn rich_flatbuffers_fixture_preserves_catalog_schema_features() {
    // Arrange
    let url = start_server().await;
    let mut c = clients(&url).await;
    create_schema(
        &mut c.schema,
        "commerce",
        "catalog",
        "main",
        "commerce_catalog.fbs",
        pb::SchemaFormat::Flatbuffers,
        COMMERCE_CATALOG_FBS,
        "fbs-rich-fixture-1",
    )
    .await;

    // Act
    let pulled = pull_source(
        &mut c.explore,
        "commerce",
        "catalog",
        "commerce_catalog.fbs",
        vref_branch("main"),
    )
    .await;
    let kinds = list_decls_with_kinds(
        &mut c.explore,
        "commerce",
        "catalog",
        "commerce_catalog.fbs",
        vref_branch("main"),
    )
    .await;
    let deps = c
        .explore
        .list_dependencies(pb::ListDependenciesRequest {
            project: "commerce".into(),
            repo: "catalog".into(),
            schema_path: "commerce_catalog.fbs".into(),
            at: Some(vref_branch("main")),
            transitive: false,
        })
        .await
        .expect("list_dependencies")
        .into_inner()
        .dependencies;

    // Assert: file metadata and rich FlatBuffers constructs survive.
    for needle in [
        "include \"common/money.fbs\"",
        "namespace commerce.catalog",
        "attribute \"priority\"",
        "enum Availability",
        "struct GeoPoint",
        "struct Transform",
        "values: [float:16]",
        "force_align: 16",
        "table Supplier",
        "table Product",
        "table Category",
        "union CatalogEntity",
        "product: Product",
        "supplier: Supplier",
        "category: Category",
        "tags: [string]",
        "related: [Product]",
        "priority: 5",
        "transform: Transform",
        "(required",
        "(deprecated",
        "(key",
        "rpc_service CatalogService",
        "root_type Product",
        "file_identifier \"CATL\"",
        "file_extension \"cat\"",
    ] {
        assert!(pulled.contains(needle), "lost `{needle}`:\n{pulled}");
    }

    // Assert: declaration kinds are classified across FlatBuffers concepts.
    let kind_of = |name: &str| -> i32 {
        kinds
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("decl `{name}` not found in {kinds:?}"))
            .1
    };
    assert_eq!(kind_of("Availability"), pb::DeclKind::FbsEnum as i32);
    assert_eq!(kind_of("GeoPoint"), pb::DeclKind::Struct as i32);
    assert_eq!(kind_of("Transform"), pb::DeclKind::Struct as i32);
    assert_eq!(kind_of("Product"), pb::DeclKind::Table as i32);
    assert_eq!(kind_of("CatalogEntity"), pb::DeclKind::Union as i32);
    assert_eq!(kind_of("CatalogService"), pb::DeclKind::Service as i32);

    // Assert: includes are tracked as dependencies.
    let dep_paths: Vec<String> = deps.into_iter().map(|d| d.import_path).collect();
    assert!(
        dep_paths.contains(&"common/money.fbs".to_string()),
        "include dependency missing: {dep_paths:?}"
    );
}
