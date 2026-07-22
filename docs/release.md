<!-- agent-updated: 2026-07-21T23:31:27Z -->
# SchemaHub Release Process

This document describes the reproducible CI and artifact contract prepared for
the 0.9 release candidate and 1.0. Publishing a tag, GitHub release, or registry
image remains an explicit user-authorized action.

## Canonical CI Matrix

`.github/workflows/ci.yml` separates failures by contract:

| Job | Contract |
|---|---|
| Rust quality | Locked metadata, formatting, strict release Clippy for all targets/features |
| Rust release suite | Redb, compilers, generated-code compilation, CLI, gRPC, HTTP BFF and generated OpenAPI, serving, JWT/JWKS identity, GUI-facing workflows |
| PostgreSQL 17 | Migrations, resource transactions, bounded load, CAS contention, distributed GC fence, restart recovery |
| GUI | Locked pnpm install, TypeScript checks, production Vite build |
| Container | PostgreSQL-capable distroless image, non-root runtime, aggregate storage/auth readiness health check, metrics/capabilities, graceful SIGTERM |

The release workflow runs only for a version tag matching
`vMAJOR.MINOR.PATCH[-PRERELEASE]`. It invokes `ci.yml` as a reusable workflow
and does not resolve metadata, build, push, or publish until every normal CI job
passes for the tagged commit.

## Compiler Dependency Coordination

SchemaHub consumes both compiler libraries at immutable Git revisions:

- `protobuf-rs`: `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac`
- `flatbuffers-rs`: `7dc2c76c08f452b9a208230057c0cb6327e65f24`

Normal CI, Cloud Build, and container builds require only the SchemaHub
checkout and resolve both compilers directly through Cargo. The lock validator
requires all three crates for each compiler to use one canonical Git URL and
revision; every cross-repository path or mutable dependency fails before a
build or release. The release metadata job separately checks out both compiler
repositories to prove that the configured revisions exist and capture
provenance. Repository variables
`PROTOBUF_RS_REF` and `FLATBUFFERS_RS_REF` select coordinated refs and may
default to `main` for ordinary development. A tag release fails before sibling
checkout unless both variables are explicit 40- or 64-character lowercase
commit SHAs. It also inspects `Cargo.lock` and requires all three crates for
each compiler to resolve from the matching canonical Git URL and revision;
cross-repository path dependencies fail the release before any build or
publication. The workflow verifies the independent checkouts match those
inputs and records them in every archive's `BUILD-METADATA.txt`.

The coordinated sibling tree passes its full default- and all-feature release
workspace suites, its normal and all-feature production-target strict CI Clippy
contracts, formatting, locked metadata, and diff hygiene.
SchemaHub's ten generated-code compilation tests also pass against it after the
runtime safety-contract cleanup. The sibling's optional `grpc` feature now maps
its local resolved schema into pinned pure-grpc codec-neutral IR without pulling
a second remote FlatBuffers schema crate; focused generation tests and an
isolated downstream crate prove the emitted owned-message codecs and unary
server/client stubs compile. SchemaHub does not enable that optional feature, so
the proof is compiler release evidence rather than SchemaHub runtime coverage.

## Production Identity Gate

The production server accepts externally issued JWTs through the explicit
`[auth.jwt]` policy documented in `authentication.md`. Unit and gRPC acceptance
tests cover strict configuration, asymmetric signature verification, issuer and
audience, injected time, human/agent mapping, atomic rotation, last-known-good
retention, and stale-key readiness.

Before a candidate is promoted, staging must also use the intended real
identity provider and immutable container digest. Complete the rotation drill,
confirm durable prefixed subjects match configured project roles, and retain
evidence for successful next-key use plus stale-key `503` behavior. Do not fall
back to static tokens for the production candidate.

## API Compatibility Gate

ADR 0002 freezes `schemahub.v1` gRPC/protobuf as the public 1.0 API. The
unversioned `/api/*` routes in the packaged OpenAPI document are GUI-only BFF
routes and are supported with the bundled GUI from the same release; they are
not marketed as public REST bindings. `/healthz`, `/readyz`, and `/metrics`
remain separately supported operational interfaces.

Candidate validation must confirm that `/api/*` responses carry
`x-schemahub-api-surface: gui-bff`, operational responses do not claim that
surface, and the packaged OpenAPI document contains the same per-path
classification and names `schemahub.v1` as the public API. A future public REST
surface requires its own versioned path, contract artifact, compatibility
declaration, and release review.

Candidate validation must also exercise each public `VersionRef` family with a
moving bookmark and confirm the response's exact immutable coordinate:
exploration/search/codegen `at_commit`, history `at_commit`, diff
`base_commit`/`head_commit`, serving revision, and ListCommits initial
`x-schemahub-at-commit` metadata. Repeat representative source, descriptor,
commit, diff, branch, and tag requests with a commit from a different repository
and require fail-closed ownership rejection. Verify that an omitted ref and an
omitted ChangeRecord target use a configured non-`main` default.

Forward traversal acceptance must resolve the exact requested field/property,
cover same-repository live, cross-repository live, and immutable pinned imports,
and assert source/target commits plus pin/path metadata. `ListDependencies` must
return normalized resolved and unresolved leaves and fail on invalid pins,
unknown formats, and bounds. Supported external OpenAPI schema, parameter,
response, and request-body component refs must additionally prove canonical
round-trip, relative-path normalization, immutable closure serving, exact
property following, reverse discovery, and live-edge deletion protection over
the public gRPC boundary. Network URLs, arbitrary fragments, repository-root
escapes, `$ref` siblings, unsupported component categories, and standalone
reference shapes that the selected AST cannot preserve remain outside the 1.0
contract and fail closed.

The frozen gRPC contract includes bounded direct reverse discovery through
`ExplorationService.ListDependents`. Candidate validation must exercise both a
live and pinned cross-repository import, retain the returned per-repository
snapshot manifest, verify a hidden repository is not disclosed, and confirm the
server advertises the 1,000-repository/10,000-schema bounds. The result is
advisory coordination data, not a global snapshot or cross-repository
transaction; see `dependency-discovery.md`.

## Published Artifact Matrix

For version `X.Y.Z`, the tag workflow prepares:

- `schemahub-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz`
- `schemahub-X.Y.Z-aarch64-apple-darwin.tar.gz`
- `schemahub-X.Y.Z-x86_64-pc-windows-msvc.tar.gz`
- `schemahub-container.spdx.json`
- `schemahub-distribution.spdx.json`
- `RELEASE-NOTES.md`
- `SHA256SUMS`
- `ghcr.io/shuozeli/schemahub:X.Y.Z` for `linux/amd64` and `linux/arm64`
- an immutable container tag `sha-<full-git-sha>` plus BuildKit provenance

Archives contain the server, CLI, README, compatibility policy, the
release-binary-generated `schemahub-http-openapi.json`, and exact
SchemaHub/compiler revisions. Release and container binaries are built with
`cargo-auditable`; the image SBOM must enumerate Rust crates as well as base OS
packages.

The tag must have a version-matched source document under `docs/releases/`.
The release workflow validates its upgrade, migration, mixed-version, rollback,
compatibility, known-issue, and provenance sections; injects the exact source,
compiler, and multi-architecture image coordinates; includes the rendered notes
in the distribution SBOM and checksums; and uses that same file as the GitHub
release body. Missing notes, unresolved template markers, or malformed
coordinates fail before publication.

The container is distroless, runs as UID/GID `65532`, includes both binaries,
and enables the PostgreSQL backend. Its default redb data directory is
`/var/lib/schemahub`; gRPC and HTTP listen inside the container on ports 50051
and 8080. Host port mappings must bind to the Tailscale interface.

## Local Artifact Rehearsal

From the shared `/home/cyuan/projects` context:

```bash
docker build \
  --file shuozeli/codegen/schemahub/Dockerfile \
  --tag schemahub:rc-rehearsal \
  --build-arg SCHEMAHUB_VERSION=0.9.0-rc.1 \
  --build-arg VCS_REF="$(git -C shuozeli/codegen/schemahub rev-parse HEAD)" \
  --build-arg BUILD_DATE="$(git -C shuozeli/codegen/schemahub show -s --format=%cI HEAD)" \
  .
```

The Dockerfile-specific ignore file admits only SchemaHub. Both compilers are
fetched at their locked Git revisions. Targets, Git metadata, GUI dependencies,
build output, and other projects never enter the build context.

To rehearse a native archive, build on the target platform with an explicit
version and package it:

```bash
export SCHEMAHUB_VERSION=0.9.0-rc.1
export SCHEMAHUB_REVISION="$(git rev-parse HEAD)"
export PROTOBUF_RS_REVISION="$(scripts/validate-compiler-lock.sh protobuf --development)"
export FLATBUFFERS_RS_REVISION="$(scripts/validate-compiler-lock.sh flatbuffers --development)"
cargo install cargo-auditable --locked --version 0.7.5
cargo auditable build --release --locked \
  --target x86_64-unknown-linux-gnu \
  -p schemahub-server -p schemahub-cli \
  --features schemahub-server/postgres
scripts/package-release.sh \
  "$SCHEMAHUB_VERSION" x86_64-unknown-linux-gnu target/release-check
```

The packaging script refuses missing binaries, a `--version` mismatch, an empty
generated OpenAPI document, mutable or malformed release versions, and
missing/non-SHA SchemaHub or compiler provenance. It never emits an archive
containing `unknown` revisions.

The JWT-enabled image was rebuilt locally with the pinned Rust 1.95 builder and
PostgreSQL feature, then exercised as UID/GID `65532` through the Tailscale
MagicDNS address. The runtime accepted a signed token, atomically replaced its
`kid`, rejected the removed key, retained last-known-good keys after a malformed
refresh, returned readiness `503` and rejected credentials after the stale
bound, made Docker health transition healthy → unhealthy → healthy, recovered
after valid keys returned, rejected a missing explicit config instead of
starting anonymously, and exited `0` on `SIGTERM`. The final local image is
`schemahub:local-jwt` (`sha256:fe043c236f856e2900fe4f2e2d081caf6d19e9470b76d3102c13f67b00f9f81c`,
24,184,963 bytes). Syft 1.48 discovered 453 packages: 443 Rust crates and 10
Debian packages. The rebuilt image also persisted a descriptor before response,
restarted over the same redb volume, and returned byte-identical content with
SHA-256 `d872b6d1aa02e5803cfda100b9943f215da8aa5241d85e25210f43a1ca9221bf`.
This is local evidence; the candidate still requires the intended real provider
in staging.

## Candidate Gate

Before creating a 0.9 tag:

1. Publish and pin clean compiler dependency commits.
2. Require every CI job on the candidate commit, including the executable
   `e2e_ga_acceptance` journey. It must preserve delegated-agent authorship,
   human review, and exact Protobuf/FlatBuffers artifact bytes across redb
   restart.
3. Verify the redb and PostgreSQL backup/restore drills.
4. Generate and inspect both SBOMs and `SHA256SUMS`.
5. Deploy the exact container digest to staging.
6. Complete the human/agent change-to-artifact workflow, restart, and compare
   exact bytes.
7. Verify an artifact first served by the prior candidate is returned
   byte-for-byte by the new candidate with the same persistent database, and
   confirm a corrupt fixture fails closed instead of rerendering.
8. Complete the real-provider JWT rotation/staleness drill in staging; static
   tokens do not satisfy this gate.
9. Exercise `ListDependents` with live, pinned, and unreadable downstream
   repositories; retain the snapshot manifest with the acceptance evidence.
10. Review `CHANGELOG.md`, migration policy, compatibility policy, known
   limitations, and the generated BFF/operations path classifications.
11. Obtain explicit authorization before pushing the tag or publishing assets.

Release notes must state the source and target versions, migration set,
mixed-version allowance, rollback window, compatibility changes, known issues,
and exact container digest. `scripts/render-release-notes.sh` enforces this
contract and the tag workflow publishes only its validated output.
