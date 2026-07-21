<!-- agent-updated: 2026-07-21T18:31:43Z -->
# Immutable Schema Serving

## Product Contract

SchemaHub separates mutable collaboration from immutable consumption. A build,
producer, consumer, migration, or data-storage service first resolves a branch,
tag, or commit to a `SchemaRevision`, then fetches artifacts only through that
pinned revision. Moving the original bookmark cannot change those bytes.
If `ResolveRevision.at` is omitted, the server uses the repository's configured
default bookmark and records that concrete branch in `resolved_from`.

Revision names are repository-scoped:

```text
projects/{project}/repos/{repo}/revisions/{commit}
```

Although JJ content objects deduplicate globally, SchemaHub verifies that the
commit is retained by the named repository's current or historical operation
views. A commit copied from another repository cannot be used to forge a
revision name and bypass repository authorization.

## gRPC API

```proto
service ServingService {
  rpc ResolveRevision(ResolveRevisionRequest) returns (SchemaRevision);
  rpc GetSchemaArtifact(GetSchemaArtifactRequest) returns (SchemaArtifact);
}
```

`GetSchemaArtifact` supports:

- `SOURCE`: canonical compiler-printed source for the requested schema.
- `DESCRIPTORS`: the format-native descriptor/bundle for its import closure.
- `GENERATED_CODE`: generated source for a supported language and options.

Every response includes the immutable revision, media type, format, dependency
schema names, payload digest, closure digest, and archive flag. Passing the
current payload digest in `if_none_match` returns metadata with empty content
and `not_modified=true`. The same digest is also returned in the
`x-schemahub-artifact-digest` gRPC metadata field.

## Digest Contracts

`artifact_digest` is:

```text
sha256(<exact response content bytes>)
```

`closure_digest` uses the versioned `schemahub-closure-v1` encoding. It hashes:

1. The explicit root `(project, repo, schema_path)`.
2. Every closure entry sorted by `SchemaPath`.
3. Each entry's file metadata blob.
4. Every declaration sorted by name, including name and blob bytes.

All strings/blobs are prefixed with an unsigned 64-bit big-endian byte length;
collection counts use the same fixed-width encoding. This makes the digest
independent of Rust `HashMap` insertion order and unambiguous across arbitrary
names and content.

The closure digest describes immutable schema input. The artifact digest also
captures compiler output, language, and code-generation options because it is
computed over the final bytes.

## First-Materialization Contract

SchemaHub durably stores the first successfully rendered artifact before
returning it. The versioned request identity covers the immutable revision,
schema path, artifact kind, generated-code language, and every relevant codegen
option. Options that cannot affect source or descriptor output are normalized
away. A new renderer input must add an explicit identity field or advance the
request-key version; it must never silently alias an existing request.

Artifacts use the `schemahub.artifacts.v1` resource collection in the same
`ObjectDb` as JJ and the control plane. The stored record contains versioned
metadata followed by the exact raw payload bytes. `create_record` is atomic on
memory, redb, and PostgreSQL, so concurrent and mixed-renderer servers converge
on one first writer. A losing writer reloads and returns the persisted winner.
Once one request succeeds, restarts and compiler/printer upgrades return those
same bytes and digests; lookup happens before compiler selection, so even a
server without that renderer can serve an existing materialization.

Reads fail closed if the record header, request identity, metadata, dependency
paths, payload digest, closure digest, or resource name is inconsistent. The
repository's current serving policy and revision ownership are checked before
lookup, and every stored dependency is reauthorized before each response.
Corruption is never repaired by silently rerendering different bytes.

Artifact records are currently retained with the database and are not swept by
JJ garbage collection. They are included by the documented redb/PostgreSQL
backup procedures. Operators must retain the named revision and artifact
records for their promised consumer window. Rolling operation with a server
that predates `schemahub.artifacts.v1`, or downgrading to one that cannot read
it, is outside the supported mixed-version window.

## HTTP Cache Semantics

The optional HTTP BFF exposes:

```text
GET /api/projects/{project}/repos/{repo}/revisions/resolve?ref=main
GET /api/projects/{project}/repos/{repo}/revisions/{commit}/artifacts/{schema_path}?kind=source
```

Artifact responses return standard quoted `ETag` plus
`X-SchemaHub-Closure-Digest`. A matching `If-None-Match` produces `304 Not
Modified` with no response body. Descriptor and generated-code variants use
`kind=descriptors` and `kind=generated-code&language=rust`.

## CLI

```bash
# Resolve once.
schemahub artifact resolve acme/commerce --at main --json

# Fetch source or binary descriptors from the returned revision.
schemahub artifact fetch \
  projects/acme/repos/commerce/revisions/<commit> \
  --schema-path order.proto --kind source

schemahub artifact fetch \
  projects/acme/repos/commerce/revisions/<commit> \
  --schema-path order.proto --kind descriptors --output order.desc

# Verify downloaded bytes against a persisted digest.
schemahub artifact verify \
  projects/acme/repos/commerce/revisions/<commit> \
  --schema-path order.proto --kind source --digest sha256:<digest>
```

`verify` recomputes SHA-256 locally and checks both the downloaded bytes and
the server-declared digest. A mismatch exits nonzero.

## Verification

The release suite covers mutable-ref pinning, source/descriptor/generated-code
artifacts, deterministic closure hashing, gRPC conditional reads, HTTP ETag
304 behavior, cross-repository commit isolation, first-writer convergence,
corruption rejection, and redb restart retrieval with the compiler registry
removed. A separate acceptance test compiles generated Rust fetched through
the immutable serving API in a fresh downstream crate.
