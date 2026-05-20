# schemahub — gRPC API Design

> This document specifies the complete gRPC API surface: all services, RPCs, request/response types, and mutation types. The `.proto` files described here are the external contract that the CLI, AI agents, and any future client libraries depend on. The internal Rust core translates these wire types into the internal `Mutation` envelope described in `design.md` Section 3.4.

---

## 1. File Structure and Package Conventions

```
proto/
  schemahub/
    v1/
      common.proto          # shared types used across multiple files
      mutations.proto       # ProtobufMutation, FlatBuffersMutation, OpenApiMutation
      schema_service.proto  # SchemaService (lifecycle + mutations)
      ref_service.proto     # RefService (branches, tags, commits, diff, merge)
      exploration_service.proto  # ExplorationService (read API)
      codegen_service.proto # CodegenService (descriptors, preview)
      project_service.proto # ProjectService (projects, repos, ACL)
      admin_service.proto   # AdminService (GC, index rebuild)
```

**Package:** `schemahub.v1`

**Go package option:** `option go_package = "github.com/shuozeli/schemahub/gen/go/schemahub/v1";`

**Rust:** generated via `tonic-build` in `build.rs`. All generated types land in the `schemahub_v1` module.

All `.proto` files use `syntax = "proto3"`.

---

## 2. `common.proto` — Shared Types

```proto
syntax = "proto3";
package schemahub.v1;

import "google/protobuf/timestamp.proto";
import "google/rpc/status.proto";

// ── Enumerations ─────────────────────────────────────────────────────────────

enum SchemaFormat {
  SCHEMA_FORMAT_UNSPECIFIED  = 0;
  SCHEMA_FORMAT_PROTOBUF     = 1;
  SCHEMA_FORMAT_FLATBUFFERS  = 2;
  SCHEMA_FORMAT_OPENAPI      = 3;
}

enum CompatibilityDirection {
  COMPATIBILITY_DIRECTION_UNSPECIFIED = 0;
  COMPATIBILITY_DIRECTION_BACKWARD    = 1;
  COMPATIBILITY_DIRECTION_FORWARD     = 2;
  COMPATIBILITY_DIRECTION_FULL        = 3;
  COMPATIBILITY_DIRECTION_DISABLED    = 4;  // no checks (dev repos)
}

enum DeclKind {
  DECL_KIND_UNSPECIFIED           = 0;
  // Protobuf
  DECL_KIND_MESSAGE               = 1;
  DECL_KIND_ENUM                  = 2;
  DECL_KIND_SERVICE               = 3;
  // FlatBuffers
  DECL_KIND_TABLE                 = 4;
  DECL_KIND_STRUCT                = 5;
  DECL_KIND_FBS_ENUM              = 6;
  DECL_KIND_UNION                 = 7;
  // OpenAPI
  DECL_KIND_PATH_ITEM             = 8;
  DECL_KIND_COMPONENT_SCHEMA      = 9;
  DECL_KIND_COMPONENT_PARAMETER   = 10;
  DECL_KIND_COMPONENT_RESPONSE    = 11;
  DECL_KIND_COMPONENT_REQUEST_BODY = 12;
  DECL_KIND_DOCUMENT_METADATA     = 13;
}

enum Role {
  ROLE_UNSPECIFIED  = 0;
  ROLE_READER       = 1;
  ROLE_WRITER       = 2;
  ROLE_MAINTAINER   = 3;
  ROLE_OWNER        = 4;
}

enum Language {
  LANGUAGE_UNSPECIFIED = 0;
  LANGUAGE_RUST        = 1;
  LANGUAGE_GO          = 2;
  LANGUAGE_TYPESCRIPT  = 3;
  LANGUAGE_PYTHON      = 4;
  LANGUAGE_JAVA        = 5;
}

// ── Core resource types ───────────────────────────────────────────────────────

// A commit in the version history.
message CommitInfo {
  string hash                           = 1;
  repeated string parent_hashes         = 2;
  google.protobuf.Timestamp timestamp   = 3;
  string author                         = 4;
  string message                        = 5;
  bool   force                          = 6;
  string format_id                      = 7;  // "protobuf", "flatbuffers", "openapi"
}

// A summary of one top-level declaration within a schema file.
message DeclSummary {
  string   name        = 1;  // e.g. "UserRequest", "path:/users", "schema:User"
  DeclKind kind        = 2;
  string   doc_comment = 3;  // first line of leading comment; empty if none
}

// A schema file within a repo.
message SchemaInfo {
  string       name      = 1;  // e.g. "user.proto"
  SchemaFormat format    = 2;
  string       head_blob = 3;  // hash of the schema tree at HEAD
}

// A branch.
message BranchInfo {
  string project     = 1;
  string repo        = 2;
  string name        = 3;
  string head_commit = 4;
  bool   protected   = 5;
}

// A tag.
message TagInfo {
  string project      = 1;
  string repo         = 2;
  string name         = 3;
  string commit_hash  = 4;  // resolved commit (even for annotated tags)
  bool   annotated    = 5;
  string tagger       = 6;  // empty for lightweight tags
  string message      = 7;  // empty for lightweight tags
  google.protobuf.Timestamp timestamp = 8;
}

// ── Version reference ─────────────────────────────────────────────────────────

// A pointer to a specific point in history. Used in read RPCs to specify
// which version to read.
message VersionRef {
  oneof ref {
    string branch = 1;  // resolves to HEAD of the branch at request time
    string tag    = 2;  // resolves to the tagged commit
    string commit = 3;  // pinned commit hash; never changes
  }
}

// ── Error detail types ────────────────────────────────────────────────────────
// These are embedded in google.rpc.Status.details as google.protobuf.Any.

// Returned when base_revision does not match current branch HEAD.
// gRPC status: FAILED_PRECONDITION
message ConflictDetail {
  string current_head   = 1;  // what the branch is actually at
  string provided_base  = 2;  // what the caller sent
  string branch         = 3;
}

// One compatibility violation found by check_compatibility.
message CompatibilityViolation {
  string                 schema_path      = 1;
  string                 declaration_name = 2;
  string                 field_name       = 3;  // empty if declaration-level
  string                 message          = 4;  // human-readable explanation
  CompatibilityDirection direction        = 5;  // the direction that was violated
}

// Returned when compatibility check fails.
// gRPC status: FAILED_PRECONDITION
message CompatibilityError {
  repeated CompatibilityViolation violations = 1;
}

// Returned when a mutation is rejected by the format validator.
// gRPC status: INVALID_ARGUMENT
message MutationValidationError {
  string schema_path      = 1;
  string declaration_name = 2;
  string field_name       = 3;
  string reason           = 4;
}

// Returned when a merge cannot fast-forward.
// gRPC status: FAILED_PRECONDITION
message MergeConflictDetail {
  string source_branch      = 1;
  string target_branch      = 2;
  string diverged_at_commit = 3;  // the LCA
  repeated string source_only_commits = 4;
  repeated string target_only_commits = 5;
}
```

---

## 3. `mutations.proto` — Mutation Operation Types

```proto
syntax = "proto3";
package schemahub.v1;

// ── Protobuf mutations ────────────────────────────────────────────────────────

// All Protobuf mutations on one schema file.
message ProtobufMutation {
  // The schema file to mutate, e.g. "user.proto"
  string schema_path = 1;

  oneof operation {
    // Field mutations
    ProtoAddField      add_field      = 2;
    ProtoRemoveField   remove_field   = 3;
    ProtoRenameField   rename_field   = 4;
    ProtoChangeFieldType change_field_type = 5;
    ProtoChangeFieldLabel change_field_label = 6;
    ProtoReorderFields reorder_fields = 7;

    // Message mutations
    ProtoAddMessage    add_message    = 10;
    ProtoRemoveMessage remove_message = 11;
    ProtoRenameMessage rename_message = 12;

    // Enum mutations
    ProtoAddEnum       add_enum       = 20;
    ProtoRemoveEnum    remove_enum    = 21;
    ProtoAddEnumValue  add_enum_value = 22;
    ProtoRemoveEnumValue remove_enum_value = 23;
    ProtoRenameEnumValue rename_enum_value = 24;

    // Service mutations
    ProtoAddService    add_service    = 30;
    ProtoRemoveService remove_service = 31;
    ProtoAddRpc        add_rpc        = 32;
    ProtoRemoveRpc     remove_rpc     = 33;
    ProtoRenameRpc     rename_rpc     = 34;

    // Import mutations
    ProtoUpdateImport  update_import  = 40;
  }
}

// ── Protobuf field mutations ──────────────────────────────────────────────────

message ProtoAddField {
  string message_name  = 1;  // the message to add a field to
  string field_name    = 2;
  // Fully qualified type name: "string", "int32", "bytes",
  // or a message/enum type like "payments.v1.Currency".
  string field_type    = 3;
  uint32 field_number  = 4;  // must not be in use or reserved
  bool   repeated      = 5;
  string doc_comment   = 6;
}

message ProtoRemoveField {
  string message_name = 1;
  string field_name   = 2;
  // Server automatically adds a reservation for this field's number and name.
  // No separate ReserveField mutation is needed.
}

message ProtoRenameField {
  string message_name   = 1;
  string old_field_name = 2;
  string new_field_name = 3;
}

message ProtoChangeFieldType {
  string message_name = 1;
  string field_name   = 2;
  // Must be wire-type compatible per the allowlist in design.md Section 4.2.
  // Cross-wire-type changes are rejected at validation time.
  string new_type     = 3;
}

message ProtoChangeFieldLabel {
  string message_name = 1;
  string field_name   = 2;
  // "optional" or "repeated". Changing from repeated to optional is breaking;
  // the validator enforces compatibility rules.
  string new_label    = 3;
}

message ProtoReorderFields {
  string message_name        = 1;
  // All field names in the desired new order. Must be a permutation of all
  // current field names. Field numbers are unchanged by reordering.
  repeated string field_order = 2;
}

// ── Protobuf message mutations ────────────────────────────────────────────────

message ProtoAddMessage {
  string message_name = 1;
  string doc_comment  = 2;
  // Fields are added via subsequent ProtoAddField mutations.
}

message ProtoRemoveMessage {
  string message_name = 1;
  // Rejected if any field in the same schema references this message type,
  // unless --force is set.
}

message ProtoRenameMessage {
  string old_name    = 1;
  string new_name    = 2;
  // Automatically updates all same-file references. Cross-file references
  // are flagged; the caller must issue UpdateImport or update referencing schemas.
}

// ── Protobuf enum mutations ───────────────────────────────────────────────────

message ProtoAddEnum {
  string enum_name   = 1;
  string doc_comment = 2;
  // The first value added must have number 0 (proto3 requirement).
}

message ProtoRemoveEnum {
  string enum_name = 1;
}

message ProtoAddEnumValue {
  string enum_name   = 1;
  string value_name  = 2;
  int32  number      = 3;
  string doc_comment = 4;
}

message ProtoRemoveEnumValue {
  string enum_name  = 1;
  string value_name = 2;
}

message ProtoRenameEnumValue {
  string enum_name      = 1;
  string old_value_name = 2;
  string new_value_name = 3;
}

// ── Protobuf service mutations ────────────────────────────────────────────────

message ProtoAddService {
  string service_name = 1;
  string doc_comment  = 2;
}

message ProtoRemoveService {
  string service_name = 1;
}

message ProtoAddRpc {
  string service_name    = 1;
  string rpc_name        = 2;
  // Fully qualified message names, e.g. "payments.v1.GetUserRequest"
  string request_type    = 3;
  string response_type   = 4;
  bool   client_streaming = 5;
  bool   server_streaming = 6;
  string doc_comment     = 7;
}

message ProtoRemoveRpc {
  string service_name = 1;
  string rpc_name     = 2;
}

message ProtoRenameRpc {
  string service_name = 1;
  string old_rpc_name = 2;
  string new_rpc_name = 3;
}

// ── Protobuf import mutations ─────────────────────────────────────────────────

message ProtoUpdateImport {
  // Logical path of the imported schema: "project/repo/schema.proto"
  string import_path  = 1;
  // Exactly one of the following must be set.
  // If none are set, re-pins to the latest commit on the imported schema's default branch.
  string to_commit    = 2;  // pin to a specific commit hash
  string to_tag       = 3;  // pin to a specific tag name
}

// ═════════════════════════════════════════════════════════════════════════════
// FlatBuffers mutations
// ═════════════════════════════════════════════════════════════════════════════

message FlatBuffersMutation {
  string schema_path = 1;  // e.g. "payments.fbs"

  oneof operation {
    // Field mutations (tables only)
    FbsAddField      add_field      = 2;
    FbsDeprecateField deprecate_field = 3;
    FbsRenameField   rename_field   = 4;
    // Note: RemoveField and ReorderFields are always rejected for FlatBuffers.

    // Table mutations
    FbsAddTable      add_table      = 10;
    FbsRemoveTable   remove_table   = 11;
    FbsRenameTable   rename_table   = 12;

    // Enum mutations
    FbsAddEnum       add_enum       = 20;
    FbsAddEnumValue  add_enum_value = 21;

    // Union mutations
    FbsAddUnion      add_union      = 30;

    // Import mutations
    FbsUpdateImport  update_import  = 40;
  }
}

// ── FlatBuffers field mutations ───────────────────────────────────────────────

message FbsAddField {
  string table_name    = 1;
  string field_name    = 2;
  // FlatBuffers scalar type or table/enum/union name.
  // e.g. "string", "int32", "float64", "bool", "PaymentStatus", "[Order]"
  string field_type    = 3;
  // Default value for scalar fields only. Stored as string, parsed by the plugin.
  // Fields at their default value are not written to the buffer.
  string default_value = 4;
  string doc_comment   = 5;
  // The field is always appended at the end of the table (highest slot index).
  // The caller cannot specify a position; the validator enforces this.
}

message FbsDeprecateField {
  // Marks the field as deprecated. Its slot index is preserved.
  // Use instead of RemoveField (which is permanently rejected for FlatBuffers).
  string table_name = 1;
  string field_name = 2;
}

message FbsRenameField {
  // Safe: FlatBuffers wire identity is slot index, not name.
  string table_name     = 1;
  string old_field_name = 2;
  string new_field_name = 3;
}

// ── FlatBuffers table mutations ───────────────────────────────────────────────

message FbsAddTable {
  string table_name  = 1;
  string doc_comment = 2;
  // Structs cannot be added via the mutation API; they are immutable once
  // defined and should be included in CreateSchema / UpdateSchema source.
}

message FbsRemoveTable {
  string table_name = 1;
}

message FbsRenameTable {
  string old_name = 1;
  string new_name = 2;
}

// ── FlatBuffers enum mutations ────────────────────────────────────────────────

message FbsAddEnum {
  string enum_name   = 1;
  // The underlying integer type: "int8", "int16", "int32", "int64",
  // "uint8", "uint16", "uint32", "uint64"
  string base_type   = 2;
  string doc_comment = 3;
}

message FbsAddEnumValue {
  string enum_name   = 1;
  string value_name  = 2;
  int64  value       = 3;
  string doc_comment = 4;
}

// ── FlatBuffers union mutations ───────────────────────────────────────────────

message FbsAddUnion {
  string          union_name   = 1;
  repeated string member_types = 2;  // table names that are union members
  string          doc_comment  = 3;
}

// ── FlatBuffers import mutations ──────────────────────────────────────────────

message FbsUpdateImport {
  string import_path = 1;  // "project/repo/schema.fbs"
  string to_commit   = 2;
  string to_tag      = 3;
}

// ═════════════════════════════════════════════════════════════════════════════
// OpenAPI mutations (v1: whole-document only)
// ═════════════════════════════════════════════════════════════════════════════

// In v1, the only OpenAPI mutation is a whole-document push.
// This is called internally by UpdateSchema; clients use UpdateSchema directly.
// Granular operations (AddOperation, RemoveParameter, etc.) are deferred to v2.
message OpenApiMutation {
  string schema_path  = 1;
  // Exactly one variant must be set.
  oneof operation {
    OpenApiPushDocument push_document = 2;
  }
}

message OpenApiPushDocument {
  // Full YAML or JSON source text of the new document version.
  string source = 1;
}
```

---

## 4. `schema_service.proto` — Schema Lifecycle and Mutations

```proto
syntax = "proto3";
package schemahub.v1;

import "schemahub/v1/common.proto";
import "schemahub/v1/mutations.proto";

service SchemaService {
  // ── Schema lifecycle ────────────────────────────────────────────────────────
  // Create a new schema file on a branch.
  // Returns ALREADY_EXISTS if the schema name exists on the branch.
  rpc CreateSchema(CreateSchemaRequest) returns (CreateSchemaResponse);

  // Update an existing schema (whole-document push for all formats;
  // also the only update path for OpenAPI in v1).
  // Returns NOT_FOUND if the schema does not exist.
  // Runs compatibility check if the branch is protected.
  rpc UpdateSchema(UpdateSchemaRequest) returns (UpdateSchemaResponse);

  // Delete a schema from a branch. Rejected if other schemas on the same branch
  // import it, unless --force is set.
  rpc DeleteSchema(DeleteSchemaRequest) returns (DeleteSchemaResponse);

  // ── Mutations ───────────────────────────────────────────────────────────────
  // Apply a single granular mutation to one declaration.
  rpc ApplyMutation(ApplyMutationRequest) returns (ApplyMutationResponse);

  // Apply a sequence of mutations across one or more schemas atomically.
  // All schemas in one transaction must use the same format.
  // Limits: ≤ 500 operations, ≤ 20 schemas, 30-second server-side timeout.
  rpc ApplyTransaction(ApplyTransactionRequest) returns (ApplyTransactionResponse);
}

// ── CreateSchema ──────────────────────────────────────────────────────────────

message CreateSchemaRequest {
  string       project     = 1;
  string       repo        = 2;
  string       branch      = 3;
  string       schema_name = 4;  // e.g. "user.proto"
  SchemaFormat format      = 5;  // required; server does not infer from content
  string       source      = 6;  // full source text
  string       base_revision    = 7;  // current HEAD commit hash (OCC)
  string       idempotency_key  = 8;
}

message CreateSchemaResponse {
  string new_commit = 1;  // hash of the commit created
}

// ── UpdateSchema ──────────────────────────────────────────────────────────────

message UpdateSchemaRequest {
  string project         = 1;
  string repo            = 2;
  string branch          = 3;
  string schema_name     = 4;
  string source          = 5;  // full source text of the new version
  string base_revision   = 6;
  string idempotency_key = 7;
  bool   force           = 8;  // skip compatibility check; requires Maintainer role
}

message UpdateSchemaResponse {
  string new_commit = 1;
}

// ── DeleteSchema ──────────────────────────────────────────────────────────────

message DeleteSchemaRequest {
  string project         = 1;
  string repo            = 2;
  string branch          = 3;
  string schema_name     = 4;
  string base_revision   = 5;
  string idempotency_key = 6;
  bool   force           = 7;  // delete even if dependents exist
}

message DeleteSchemaResponse {
  string new_commit = 1;
}

// ── ApplyMutation ─────────────────────────────────────────────────────────────

message ApplyMutationRequest {
  string project         = 1;
  string repo            = 2;
  string branch          = 3;
  string base_revision   = 4;
  string idempotency_key = 5;
  bool   force           = 6;
  oneof operation {
    ProtobufMutation    protobuf_op = 7;
    FlatBuffersMutation fbs_op      = 8;
    OpenApiMutation     openapi_op  = 9;
  }
}

message ApplyMutationResponse {
  string new_commit = 1;
}

// ── ApplyTransaction ──────────────────────────────────────────────────────────

message ApplyTransactionRequest {
  string project         = 1;
  string repo            = 2;
  string branch          = 3;
  string base_revision   = 4;
  string idempotency_key = 5;
  bool   force           = 6;
  // All operations must target the same format.
  // Mixed-format transactions are rejected with INVALID_ARGUMENT.
  repeated TransactionOp operations = 7;
}

// One operation within a transaction. The oneof mirrors ApplyMutationRequest
// but without the top-level fields (project, repo, branch) which are shared.
message TransactionOp {
  oneof operation {
    ProtobufMutation    protobuf_op = 1;
    FlatBuffersMutation fbs_op      = 2;
    // OpenApi granular ops are v2; use UpdateSchema for OpenAPI in v1.
  }
}

message ApplyTransactionResponse {
  string new_commit = 1;
}
```

---

## 5. `ref_service.proto` — Version Control

```proto
syntax = "proto3";
package schemahub.v1;

import "google/protobuf/timestamp.proto";
import "schemahub/v1/common.proto";

service RefService {
  // ── Commits ─────────────────────────────────────────────────────────────────
  rpc GetCommit(GetCommitRequest) returns (GetCommitResponse);
  // Streams commits in reverse chronological order (newest first).
  // Terminate the stream when the client has received enough (standard gRPC cancellation).
  rpc ListCommits(ListCommitsRequest) returns (stream CommitInfo);
  // Returns the semantic diff between two refs (or two commits).
  rpc Diff(DiffRequest) returns (DiffResponse);

  // ── Branches ────────────────────────────────────────────────────────────────
  rpc CreateBranch(CreateBranchRequest) returns (CreateBranchResponse);
  rpc DeleteBranch(DeleteBranchRequest) returns (DeleteBranchResponse);
  rpc ListBranches(ListBranchesRequest) returns (ListBranchesResponse);
  rpc GetBranch(GetBranchRequest) returns (GetBranchResponse);

  // ── Tags ─────────────────────────────────────────────────────────────────────
  rpc CreateTag(CreateTagRequest) returns (CreateTagResponse);
  rpc DeleteTag(DeleteTagRequest) returns (DeleteTagResponse);
  rpc ListTags(ListTagsRequest) returns (ListTagsResponse);

  // ── Merge ────────────────────────────────────────────────────────────────────
  // Fast-forward merge only in v1.
  // Returns MergeConflictDetail in status.details if not fast-forwardable.
  rpc Merge(MergeRequest) returns (MergeResponse);
}

// ── Commits ───────────────────────────────────────────────────────────────────

message GetCommitRequest {
  string project = 1;
  string repo    = 2;
  string commit  = 3;  // commit hash
}

message GetCommitResponse {
  CommitInfo commit = 1;
}

message ListCommitsRequest {
  string     project = 1;
  string     repo    = 2;
  VersionRef from    = 3;  // start walking from here (default: HEAD of default branch)
  // Optional: stop when this ref is reached (exclusive). Used to list only the
  // commits on a feature branch: from=feature/xyz, stop_at=main.
  string     stop_at_commit = 4;
  // Optional: filter to commits that touched a specific schema file.
  string     schema_path    = 5;
}
// Response: stream of CommitInfo, newest first.

message DiffRequest {
  string project = 1;
  string repo    = 2;
  // Two sides of the diff. Typically "base" is main and "head" is a feature branch.
  VersionRef base = 3;
  VersionRef head = 4;
  // Optional: restrict diff to one schema file.
  string schema_path = 5;
}

message DiffResponse {
  repeated SchemaDiff schema_diffs = 1;
}

message SchemaDiff {
  string schema_path   = 1;
  // Each entry is a DeclarationChange for one top-level declaration.
  repeated DeclarationChange changes = 2;
}

message DeclarationChange {
  string change_type = 1;  // "added", "removed", "modified"
  string decl_name   = 2;
  // Format-specific change detail, opaque to the client UI layer.
  // Deserialize with the appropriate client-side plugin library.
  bytes  detail      = 3;
}

// ── Branches ──────────────────────────────────────────────────────────────────

message CreateBranchRequest {
  string project   = 1;
  string repo      = 2;
  string name      = 3;
  // Start the branch from this ref. Defaults to the default branch HEAD.
  VersionRef from  = 4;
}

message CreateBranchResponse {
  BranchInfo branch = 1;
}

message DeleteBranchRequest {
  string project = 1;
  string repo    = 2;
  string name    = 3;
  // Protected branches cannot be deleted; returns FAILED_PRECONDITION.
}

message DeleteBranchResponse {}

message ListBranchesRequest {
  string project      = 1;
  string repo         = 2;
  string name_prefix  = 3;  // optional filter; e.g. "feature/" to list feature branches
}

message ListBranchesResponse {
  repeated BranchInfo branches = 1;
}

message GetBranchRequest {
  string project = 1;
  string repo    = 2;
  string name    = 3;
}

message GetBranchResponse {
  BranchInfo branch = 1;
}

// ── Tags ──────────────────────────────────────────────────────────────────────

message CreateTagRequest {
  string     project  = 1;
  string     repo     = 2;
  string     name     = 3;
  VersionRef target   = 4;  // commit/branch/tag to tag
  // If message is set, an annotated tag object is created.
  // If message is empty, a lightweight tag (ref only) is created.
  string     message  = 5;
}

message CreateTagResponse {
  TagInfo tag = 1;
}

message DeleteTagRequest {
  string project = 1;
  string repo    = 2;
  string name    = 3;
  // Requires --force equivalent (force=true) because tags are immutable.
  // Returns FAILED_PRECONDITION if force is not set.
  bool   force   = 4;
}

message DeleteTagResponse {}

message ListTagsRequest {
  string project     = 1;
  string repo        = 2;
  string name_prefix = 3;
}

message ListTagsResponse {
  repeated TagInfo tags = 1;
}

// ── Merge ─────────────────────────────────────────────────────────────────────

message MergeRequest {
  string project         = 1;
  string repo            = 2;
  string source_branch   = 3;  // the branch to merge in
  string target_branch   = 4;  // the branch to merge into
  string base_revision   = 5;  // current HEAD of target_branch (OCC)
  string idempotency_key = 6;
  string message         = 7;  // optional; used in commit message if a merge commit is created
}

message MergeResponse {
  // The commit that target_branch now points to after the merge.
  // For fast-forward: this is the former HEAD of source_branch.
  string new_commit = 1;
}
```

---

## 6. `exploration_service.proto` — Read API

```proto
syntax = "proto3";
package schemahub.v1;

import "schemahub/v1/common.proto";

service ExplorationService {
  // List all schema files in a repo at a given ref.
  rpc ListSchemas(ListSchemasRequest) returns (ListSchemasResponse);

  // List all top-level declarations in a schema file.
  rpc ListDeclarations(ListDeclarationsRequest) returns (ListDeclarationsResponse);

  // Return full detail for one named declaration.
  rpc GetDeclaration(GetDeclarationRequest) returns (GetDeclarationResponse);

  // Follow a field's type reference, potentially crossing import boundaries.
  rpc FollowType(FollowTypeRequest) returns (FollowTypeResponse);

  // List all schemas imported by a schema, at their pinned resolved_commit.
  rpc ListDependencies(ListDependenciesRequest) returns (ListDependenciesResponse);

  // Search for declarations by name across schemas and repos.
  rpc Search(SearchRequest) returns (SearchResponse);
}

// ── ListSchemas ───────────────────────────────────────────────────────────────

message ListSchemasRequest {
  string     project = 1;
  string     repo    = 2;
  VersionRef at      = 3;  // defaults to HEAD of default branch
}

message ListSchemasResponse {
  repeated SchemaInfo schemas = 1;
}

// ── ListDeclarations ──────────────────────────────────────────────────────────

message ListDeclarationsRequest {
  string     project     = 1;
  string     repo        = 2;
  string     schema_path = 3;  // e.g. "user.proto"
  VersionRef at          = 4;
  // Optional: filter by kind.
  DeclKind   kind_filter = 5;
}

message ListDeclarationsResponse {
  repeated DeclSummary declarations = 1;
}

// ── GetDeclaration ────────────────────────────────────────────────────────────

message GetDeclarationRequest {
  string     project          = 1;
  string     repo             = 2;
  string     schema_path      = 3;
  string     declaration_name = 4;  // e.g. "UserRequest", "path:/users"
  VersionRef at               = 5;
}

message GetDeclarationResponse {
  DeclSummary summary    = 1;
  // Full detail as format-specific bytes. The client-side plugin library
  // deserializes this for display. The CLI renders it as human-readable text.
  bytes       detail     = 2;
  // The commit hash that was actually resolved (useful when `at` was a branch name).
  string      at_commit  = 3;
}

// ── FollowType ────────────────────────────────────────────────────────────────

message FollowTypeRequest {
  string     project          = 1;
  string     repo             = 2;
  string     schema_path      = 3;
  string     declaration_name = 4;  // the declaration containing the field
  string     field_name       = 5;  // the field whose type to follow
  VersionRef at               = 6;
}

message FollowTypeResponse {
  // The schema and declaration where the field's type is defined.
  // May be in a different project/repo (cross-repo import).
  string resolved_project     = 1;
  string resolved_repo        = 2;
  string resolved_schema_path = 3;
  string resolved_commit      = 4;  // the pinned import commit that was followed
  DeclSummary summary         = 5;
  bytes       detail          = 6;
}

// ── ListDependencies ──────────────────────────────────────────────────────────

message ListDependenciesRequest {
  string     project     = 1;
  string     repo        = 2;
  string     schema_path = 3;
  VersionRef at          = 4;
  // If true, return the transitive closure (all transitive imports).
  // If false (default), return only direct imports.
  bool       transitive  = 5;
}

message ListDependenciesResponse {
  repeated DependencyEntry dependencies = 1;
}

message DependencyEntry {
  string importing_schema  = 1;
  string importing_decl    = 2;
  string imported_project  = 3;
  string imported_repo     = 4;
  string imported_schema   = 5;
  string imported_decl     = 6;
  string resolved_commit   = 7;  // the pinned commit hash
}

// ── Search ────────────────────────────────────────────────────────────────────

message SearchRequest {
  string   query   = 1;  // declaration name prefix to search for
  // Optional scope filters. If omitted, searches all projects/repos visible to caller.
  string   project = 2;
  string   repo    = 3;
  DeclKind kind    = 4;  // filter by kind; UNSPECIFIED = all kinds
  uint32   limit   = 5;  // max results; default 50, max 200
}

message SearchResponse {
  repeated SearchResult results = 1;
}

message SearchResult {
  string      project     = 1;
  string      repo        = 2;
  string      schema_path = 3;
  DeclSummary declaration = 4;
}
```

---

## 7. `codegen_service.proto` — Descriptors and Code Generation

```proto
syntax = "proto3";
package schemahub.v1;

import "schemahub/v1/common.proto";

service CodegenService {
  // Returns the schema in its native descriptor format, reconstructed from
  // the AST. Includes all transitive imports.
  //   Protobuf    → serialized FileDescriptorSet (binary proto)
  //   FlatBuffers → bundle of reconstructed .fbs source files (as a tar archive)
  //   OpenAPI     → resolved YAML document with all $ref inlined
  rpc GetDescriptors(GetDescriptorsRequest) returns (GetDescriptorsResponse);

  // Renders generated code for a given language server-side.
  // No files written; response contains the rendered source text.
  // Returns UNIMPLEMENTED for language/format combinations not yet supported.
  rpc PreviewCodegen(PreviewCodegenRequest) returns (PreviewCodegenResponse);
}

message GetDescriptorsRequest {
  string     project     = 1;
  string     repo        = 2;
  string     schema_path = 3;
  VersionRef at          = 4;
}

message GetDescriptorsResponse {
  // The raw descriptor artifact bytes.
  // Consumers should use the format field to interpret the bytes correctly.
  bytes        descriptor_bytes = 1;
  SchemaFormat format           = 2;
  // The commit that was actually resolved.
  string       at_commit        = 3;
}

message PreviewCodegenRequest {
  string     project     = 1;
  string     repo        = 2;
  string     schema_path = 3;
  VersionRef at          = 4;
  Language   language    = 5;
}

message PreviewCodegenResponse {
  // Generated source text. For multi-file outputs, this is a tar archive.
  // Single-file outputs (e.g. a single .rs file) are returned directly as UTF-8.
  bytes  content       = 1;
  bool   is_archive    = 2;   // true if content is a tar archive
  string at_commit     = 3;
}
```

---

## 8. `project_service.proto` — Projects, Repos, and ACL

```proto
syntax = "proto3";
package schemahub.v1;

import "schemahub/v1/common.proto";

service ProjectService {
  // ── Projects ─────────────────────────────────────────────────────────────────
  rpc CreateProject(CreateProjectRequest) returns (CreateProjectResponse);
  rpc GetProject(GetProjectRequest)       returns (GetProjectResponse);
  rpc ListProjects(ListProjectsRequest)   returns (ListProjectsResponse);
  rpc DeleteProject(DeleteProjectRequest) returns (DeleteProjectResponse);

  // ── Repos ────────────────────────────────────────────────────────────────────
  rpc CreateRepo(CreateRepoRequest) returns (CreateRepoResponse);
  rpc GetRepo(GetRepoRequest)       returns (GetRepoResponse);
  rpc UpdateRepo(UpdateRepoRequest) returns (UpdateRepoResponse);
  rpc ListRepos(ListReposRequest)   returns (ListReposResponse);
  rpc DeleteRepo(DeleteRepoRequest) returns (DeleteRepoResponse);

  // ── Members and ACL ───────────────────────────────────────────────────────────
  rpc AddMember(AddMemberRequest)         returns (AddMemberResponse);
  rpc RemoveMember(RemoveMemberRequest)   returns (RemoveMemberResponse);
  rpc UpdateMemberRole(UpdateMemberRoleRequest) returns (UpdateMemberRoleResponse);
  rpc ListMembers(ListMembersRequest)     returns (ListMembersResponse);
}

// ── Project messages ──────────────────────────────────────────────────────────

message ProjectInfo {
  string name       = 1;
  bool   is_public  = 2;  // public projects: read RPCs require no auth
  string owner      = 3;  // identity of the Owner role member
}

message CreateProjectRequest {
  string name      = 1;
  bool   is_public = 2;
}

message CreateProjectResponse {
  ProjectInfo project = 1;
}

message GetProjectRequest {
  string name = 1;
}

message GetProjectResponse {
  ProjectInfo project = 1;
}

message ListProjectsRequest {
  // If empty, returns all projects visible to the caller.
  string name_prefix = 1;
}

message ListProjectsResponse {
  repeated ProjectInfo projects = 1;
}

message DeleteProjectRequest {
  string name = 1;
  // Requires Owner role. Fails if the project has repos unless force=true.
  bool   force = 2;
}

message DeleteProjectResponse {}

// ── Repo messages ─────────────────────────────────────────────────────────────

message RepoConfig {
  string                 project                  = 1;
  string                 name                     = 2;
  string                 default_branch           = 3;  // e.g. "main"
  CompatibilityDirection compatibility_direction  = 4;
  repeated string        protected_branches       = 5;  // supports glob patterns
}

message CreateRepoRequest {
  string                 project                 = 1;
  string                 name                    = 2;
  string                 default_branch          = 3;  // default: "main"
  CompatibilityDirection compatibility_direction = 4;  // default: FULL
  repeated string        protected_branches      = 5;  // default: ["main"]
}

message CreateRepoResponse {
  RepoConfig repo = 1;
}

message GetRepoRequest {
  string project = 1;
  string repo    = 2;
}

message GetRepoResponse {
  RepoConfig repo = 1;
}

message UpdateRepoRequest {
  string                 project                 = 1;
  string                 repo                    = 2;
  // Only set fields that should change.
  CompatibilityDirection compatibility_direction = 3;
  repeated string        protected_branches      = 4;  // full replacement; empty = no change
  string                 default_branch          = 5;
}

message UpdateRepoResponse {
  RepoConfig repo = 1;
}

message ListReposRequest {
  string project     = 1;
  string name_prefix = 2;
}

message ListReposResponse {
  repeated RepoConfig repos = 1;
}

message DeleteRepoRequest {
  string project = 1;
  string repo    = 2;
  // Fails if the repo has schemas unless force=true.
  bool   force   = 3;
}

message DeleteRepoResponse {}

// ── Member/ACL messages ───────────────────────────────────────────────────────

message MemberEntry {
  string identity = 1;  // user identifier (opaque string; format depends on AuthnProvider)
  Role   role     = 2;
}

message AddMemberRequest {
  string   project  = 1;
  string   identity = 2;
  Role     role     = 3;
}

message AddMemberResponse {
  MemberEntry member = 1;
}

message RemoveMemberRequest {
  string project  = 1;
  string identity = 2;
}

message RemoveMemberResponse {}

message UpdateMemberRoleRequest {
  string project  = 1;
  string identity = 2;
  Role   new_role = 3;
}

message UpdateMemberRoleResponse {
  MemberEntry member = 1;
}

message ListMembersRequest {
  string project = 1;
}

message ListMembersResponse {
  repeated MemberEntry members = 1;
}
```

---

## 9. `admin_service.proto` — Operational RPCs

```proto
syntax = "proto3";
package schemahub.v1;

service AdminService {
  // Run garbage collection. Removes unreachable objects older than the
  // configured age threshold. Non-blocking: returns immediately with stats.
  rpc RunGC(RunGCRequest) returns (RunGCResponse);

  // Rebuild the deps/ and index/ derived indices by scanning all reachable blobs.
  // Use when an index is suspected to be corrupted or out of sync.
  // This is a slow operation on large repos.
  rpc RebuildIndex(RebuildIndexRequest) returns (RebuildIndexResponse);

  // Returns the live server configuration (limits, thresholds, backend info).
  rpc GetServerConfig(GetServerConfigRequest) returns (GetServerConfigResponse);
}

message RunGCRequest {
  // Optional: restrict GC to objects belonging to one project/repo.
  // If empty, GC runs across all projects/repos.
  string project = 1;
  string repo    = 2;
  // If true, report what would be deleted without deleting anything.
  bool   dry_run = 3;
}

message RunGCResponse {
  uint64 objects_scanned   = 1;
  uint64 objects_deleted   = 2;
  uint64 bytes_reclaimed   = 3;
  uint64 pending_entries_cleaned = 4;
  uint64 idempotency_entries_cleaned = 5;
}

message RebuildIndexRequest {
  string project = 1;
  string repo    = 2;
}

message RebuildIndexResponse {
  uint64 blobs_scanned     = 1;
  uint64 index_entries_written = 2;
  uint64 deps_entries_written  = 3;
}

message GetServerConfigRequest {}

message GetServerConfigResponse {
  uint32 max_ops_per_transaction    = 1;
  uint32 max_schemas_per_transaction = 2;
  uint32 transaction_timeout_secs   = 3;
  uint32 pending_cleanup_threshold_secs = 4;
  uint32 idempotency_ttl_hours      = 5;
  uint32 gc_age_threshold_hours     = 6;
  string storage_backend            = 7;  // e.g. "redb"
  string server_version             = 8;
}
```

---

## 10. Authentication

Authentication is transport-level, not encoded in the proto types.

**Token auth (default):** The caller includes an `Authorization: Bearer <token>` metadata header. The `AuthnProvider` trait implementation extracts the token and resolves it to an `Identity`.

**No-auth mode (getting started):** The no-op `AuthnProvider` returns `Identity::Anonymous` for all requests, and the no-op `AuthzPolicy` allows all operations. The server starts in no-auth mode when no auth configuration is provided.

**gRPC metadata key:** `authorization` (lowercase, as required by the HTTP/2 / gRPC metadata spec).

There are no auth-specific RPCs in the API — auth configuration is managed via the server config file (`schemahub.toml`), not via gRPC.

---

## 11. Error Handling

### Status codes used

| gRPC Status | When |
|-------------|------|
| `OK` | Success |
| `NOT_FOUND` | Project, repo, schema, branch, tag, or declaration does not exist |
| `ALREADY_EXISTS` | `CreateSchema` on an existing schema name; `CreateBranch` with an existing name |
| `INVALID_ARGUMENT` | Missing required field; transaction exceeds limits; mixed-format transaction; unsupported language for `PreviewCodegen` |
| `FAILED_PRECONDITION` | Base revision mismatch (`ConflictDetail`); compatibility violation (`CompatibilityError`); merge not fast-forwardable (`MergeConflictDetail`); deleting protected branch |
| `PERMISSION_DENIED` | AuthZ check failed (wrong role) |
| `UNAUTHENTICATED` | AuthN failed (invalid or missing token) |
| `RESOURCE_EXHAUSTED` | Transaction timeout exceeded (`DeadlineExceeded` behavior) |
| `INTERNAL` | Server bug |
| `UNIMPLEMENTED` | Language/format combination not supported in `PreviewCodegen` |

### Rich error details

All `FAILED_PRECONDITION` errors include structured details in `google.rpc.Status.details` (as `google.protobuf.Any`). The type URL identifies the detail type:

```
type.googleapis.com/schemahub.v1.ConflictDetail
type.googleapis.com/schemahub.v1.CompatibilityError
type.googleapis.com/schemahub.v1.MergeConflictDetail
type.googleapis.com/schemahub.v1.MutationValidationError
```

The CLI unpacks these details and renders them as human-readable output. AI agents can inspect `CompatibilityError.violations` to understand which exact change caused a failure and decide whether to retry with `--force` or reformulate the mutation.

---

## 12. Design Decisions

**One service per concern, not one mega-service.** Six services keeps each `.proto` file focused and the generated Rust traits manageable. Clients that only need the read API import only `ExplorationService`; clients doing schema mutations only import `SchemaService`.

**`stream CommitInfo` for `ListCommits`.** Commit history can be unbounded. Server streaming lets the client cancel after receiving the commits it needs without the server materializing the full history.

**`VersionRef` uses `oneof`, not a single string.** A single string like `"main"` or `"v1.0.0"` is ambiguous — is it a branch or a tag? `oneof` forces the caller to declare intent explicitly. This also makes the server's resolution logic unambiguous.

**`format` is always explicit.** No content-type inference. The CLI infers from file extension and always sets the field. This prevents format misdetection from silently producing wrong ASTs.

**`base_revision` and `idempotency_key` are on every write RPC.** This makes OCC and idempotency opt-out impossible — callers must always provide both. Clients that don't care about retries generate a fresh UUID for `idempotency_key` and set `base_revision` to the latest HEAD they have.

**No pagination tokens for list RPCs.** In v1, list operations return complete results. Schema counts are bounded (a repo with 10,000 schema files is pathological). If pagination becomes necessary, it's added in v2 — the response messages have no `next_page_token` field to accidentally leave empty.

**`OpenApiMutation` wraps `PushDocument` in v1.** Rather than removing the `OpenApiMutation` wrapper entirely, keeping the wrapper allows v2 to add granular operations (`AddOperation`, `RemoveParameter`) inside the same oneof without changing the top-level `ApplyMutationRequest` structure. No client breakage.
