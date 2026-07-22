<!-- agent-updated: 2026-07-21T20:46:16Z -->
# Format Capability Contract

SchemaHub publishes an executable, versioned description of the schema-format
workflows supported by the running server. Clients should query this contract
instead of inferring support from protobuf message presence or documentation.

```bash
schemahub capabilities
schemahub capabilities --json
```

The CLI calls `AdminService.GetFormatCapabilities`. Its JSON form is the stable
machine-readable representation. `matrix_version` is currently `1.0`; it changes
only when the interpretation of the matrix changes incompatibly. Adding an
operation does not require a version bump.

## Format-level capabilities

| Format | Parse / print | Compatibility | Conflicts | Descriptor artifact | Generated code |
|---|---:|---:|---:|---:|---|
| Protobuf | Yes | Yes | Yes | Yes | Rust |
| FlatBuffers | Yes | Yes | Yes | Yes | Rust, TypeScript |
| OpenAPI 3.x | Yes | Yes | Yes | Yes | None |

`descriptor_artifact` means the immutable serving plane can materialize the
format's native resolved artifact: a Protobuf `FileDescriptorSet`, a reconstructed
FlatBuffers bundle, or resolved OpenAPI YAML. OpenAPI source and descriptors are
served, but generated client/server code is outside the 1.0 scope.

## Mutation operations

Every operation marked **supported** below is available through both
`ApplyMutation` and `ApplyTransaction`. A transaction applies its ordered edits
atomically and validates reference integrity against the final state.

| Format | Supported operations |
|---|---|
| Protobuf | `add_field`, `remove_field`, `rename_field`, `change_field_type`, `change_field_label`, `reorder_fields`, `add_message`, `remove_message`, `rename_message`, `add_enum`, `remove_enum`, `add_enum_value`, `remove_enum_value`, `rename_enum_value`, `add_service`, `remove_service`, `rename_service`, `add_rpc`, `remove_rpc`, `rename_rpc`, `change_rpc_type`, `update_import` |
| FlatBuffers | `add_field`, `deprecate_field`, `rename_field`, `change_field_type`, `add_table`, `remove_table`, `rename_table`, `add_enum`, `remove_enum`, `rename_enum`, `add_enum_value`, `remove_enum_value`, `rename_enum_value`, `add_union`, `remove_union`, `rename_union`, `add_union_member`, `remove_union_member`, `update_import` |
| OpenAPI | `push_document`, `add_path`, `remove_path`, `add_operation`, `remove_operation`, `add_component_schema`, `remove_component_schema` |

FlatBuffers `remove_field` and `reorder_fields` are explicitly **rejected**, not
silently omitted. A field's slot is its wire identity; deprecate an obsolete
field and append new fields instead.

## Imports and immutable pins

Protobuf and FlatBuffers `update_import` can add, update, or remove an import.
An added or updated import may provide either `to_commit` or `to_tag`, never
both. A tag is resolved by the server immediately and the resulting immutable
commit ID is stored in the schema metadata; later tag changes cannot change the
dependency used by an existing revision.

OpenAPI external schema, parameter, response, and request-body component `$ref`
values in the selected 1.0 AST are discovered from declaration bodies. They use
`<schema-path>#/components/<category>/<name>` and become live/unpinned imports;
there is no OpenAPI text operation for storing an immutable commit pin.
Same-repository edges resolve on the importing revision, and a
cross-repository live target is captured once per call. Immutable serving then
persists first-materialized bytes and their closure digest. Network URLs,
absolute/query-bearing references, arbitrary fragments, `$ref` sibling fields,
and unsupported standalone reference shapes fail ingest rather than being
mistaken for registry coordinates or losing constraints. Other OpenAPI
component categories are outside the 1.0 dependency guarantee. Explicit `./`
and `../` paths resolve against the importing schema directory and cannot
escape the repository root.

Pinned imports use `project/repo/schema-file` paths. The server verifies that
the pinned commit belongs to that repository and that the target schema exists
at the commit. Unpinned local-format imports remain valid for same-repository
source layouts. A full cross-repository path without a resolved commit is a live
edge: discovery reports it as unpinned, but an independent provider publication
can make it drift or break. Durable data workflows should prefer immutable pins.

## Reference integrity

- Protobuf message and enum-value renames update same-file field, extension,
  RPC, and proto2 default references as applicable. Deleting a still-referenced
  declaration or enum value is rejected.
- FlatBuffers table, enum, union, and enum-value renames update same-file field,
  union-member, service, root-type, default-value, and declaration-order
  metadata where applicable. Referenced deletions are rejected.
- OpenAPI component-schema removal is rejected while a local `$ref` still
  targets it. Whole-schema deletion also rejects any remaining supported live
  external OpenAPI component ref, and external property `FollowType` resolves
  the exact imported declaration and immutable snapshot.
- Batch mutations validate the final schema state. A transaction may therefore
  remove a dependency before removing or rewriting its consumer later in the
  same atomic batch.
- Whole-schema lifecycle and ChangeRecord deletion reject remaining
  same-repository live unpinned imports. `force` bypasses compatibility only;
  it cannot create a dangling import. Immutable commit pins remain valid after
  the provider disappears from a mutable bookmark.

Cross-repository rename propagation is not automatic. `ListDependents` scans
all repositories readable by the caller for direct imports and returns pin state
plus the immutable commit inspected for each repository. Callers retain that
manifest and submit explicit downstream ChangeRecords. The scan is bounded and
authorization-filtered, and it is not a global transaction or transitive reverse
graph. See `dependency-discovery.md`.

## Compatibility and evolution

The capability matrix describes reachability, not whether a particular edit is
wire-compatible. Protected bookmarks still run the format compiler's
compatibility rules; incompatible edits require repository policy to allow an
authorized force operation.

When adding or removing an operation, update the live matrix, its end-to-end
reachability test, this document, and the relevant format behavior tests in the
same change. Unsupported operations must return a structured error and must not
be advertised as supported.
