<!-- agent-updated: 2026-07-30T04:23:54Z -->
# SchemaHub Release Process

This document describes the reproducible CI and artifact contract prepared for
the 0.9 release candidate and 1.0. Publishing a tag, GitHub release, or registry
image remains an explicit user-authorized action.

## Canonical CI Matrix

`.github/workflows/ci.yml` separates failures by contract:

| Job | Contract |
|---|---|
| Rust quality | Locked metadata, checksummed and self-audited RustSec tool bootstrap, dependency-policy contracts, pinned zero-vulnerability/exact-warning SchemaHub audit, formatting, strict release Clippy for all targets/features |
| Rust release suite | Redb, compilers, generated-code compilation, CLI, gRPC, HTTP BFF/OpenAPI, serving, JWT/JWKS identity, seven real-world codelabs, and the retained GA report |
| PostgreSQL 17 | Migrations, resource transactions, bounded load, CAS contention, distributed GC fence, restart recovery |
| GUI | Locked pnpm install and Low-severity audit, TypeScript checks, lazy self-contained production Vite build, entry-bundle and runtime-CDN rejection, mock executable-edit authoring smoke, and live Chromium agent-author/human-review/agent-Apply/restart acceptance against the HTTP BFF and redb |
| Workflow demo | Locked pnpm install and Low-severity audit, TypeScript checks, static Sites bundle, and real workerd boot/runtime smoke |
| Container | Digest-pinned Node/Rust/distroless bases, PostgreSQL-capable non-root runtime, named-volume write and replacement recovery, immutable descriptor/generated-code verification, bundled GUI/BFF routing, aggregate storage/auth readiness health check, metrics/capabilities, graceful SIGTERM |

The release workflow runs only for a version tag matching
`vMAJOR.MINOR.PATCH[-PRERELEASE]`. It invokes `ci.yml` as a reusable workflow
and does not resolve metadata, build, push, or publish until every normal CI job
passes for the tagged commit.

## Compiler Dependency Coordination

SchemaHub consumes both compiler libraries at immutable Git revisions:

- `protobuf-rs`: `a7cb7c6d54d79bd6029278a36f1ad6f5aacdf8ac`
- `flatbuffers-rs`: `59756d23993538b722f68675c35129c3cebb7aa1`

Normal CI, Cloud Build, and container builds require only the SchemaHub
checkout and resolve both compilers directly through Cargo. The lock validator
requires all three crates for each compiler to use one canonical Git URL and
revision; a path or mutable coordinate for any of those six direct compiler
crates fails before a build or release. The complete transitive graph is fixed
by `Cargo.lock` and every production build uses `--locked`. The release metadata
job separately checks out both compiler repositories to prove that the
configured revisions exist and capture provenance. Repository variables
`PROTOBUF_RS_REF` and `FLATBUFFERS_RS_REF` select coordinated refs and may
default to `main` for ordinary development. A tag release fails before sibling
checkout unless both variables are explicit 40- or 64-character lowercase
commit SHAs. It also inspects `Cargo.lock` and requires all three crates for
each compiler to resolve from the matching canonical Git URL and revision;
cross-repository path dependencies fail the release before any build or
publication. The workflow verifies the independent checkouts match those
inputs and records them in every archive's `BUILD-METADATA.txt`.

The warning-clean FlatBuffers follow-up is published and pinned at
`59756d23993538b722f68675c35129c3cebb7aa1`. It uses associated
`Type::create` constructors, omits deprecated fields from write/debug/serde
surfaces, and generates struct-union readers through the fully qualified
`Follow` trait and public `Table` accessors. The full default/all-feature
release suites, normal/all-feature production-target strict Clippy, formatting,
generated-code compile tests, and the dedicated `#![deny(warnings)]`
downstream crate passed both locally and in FlatBuffers main Actions run
`30481753669`; `RW-03-001` is fixed.

SchemaHub's current `Cargo.toml` and `Cargo.lock` resolve all three FlatBuffers
compiler crates from that exact canonical Git coordinate. The live
`FLATBUFFERS_RS_REF` repository variable was advanced on 2026-07-30 to the
same exact revision; `PROTOBUF_RS_REF` likewise matches the Protobuf coordinate
in `Cargo.lock`. Publishing the current SchemaHub tree and obtaining a clean
candidate run remain separate gates. The sibling's optional `grpc` feature
also maps its resolved schema into
pinned pure-grpc codec-neutral IR without pulling a second remote FlatBuffers
schema crate; focused generation tests and an isolated downstream crate prove
the emitted owned-message codecs and unary server/client stubs compile.
SchemaHub does not enable that optional feature, so the proof is compiler
release evidence rather than SchemaHub runtime coverage.

The 2026-07-29 GitHub audit found draft PR 4 still mergeable at pushed source
`a8b3c5f29d6aa91f5cd0e4ab9ad5c4fea7b1e844`; its historical six-job CI run
`30019762296` is green. Both workflow files and the current 1.0 source are
newer local changes, so those checks are not candidate evidence. The repository
still has no environments, deployments, Actions secret names, tags, or
releases.

## Locked Dependency Security Gate

Release CI uses `scripts/install-cargo-audit.sh` to verify cargo-audit 0.22.2's
exact crates.io source archive, replace its published lock with the reviewed
`tools/cargo-audit/Cargo.lock`, install that graph with `--locked`, and audit
the auditor's own lock before it is trusted. The source archive and reviewed
lock have separate pinned SHA-256 identities. The published cargo-audit lock is
deliberately not used: it resolves vulnerable `crossbeam-epoch` 0.9.18 and
`quinn-proto` 0.11.14 plus unsound `anyhow` 1.0.102 and `memmap2` 0.9.10. The
reviewed graph instead resolves patched `crossbeam-epoch` 0.9.20,
`quinn-proto` 0.11.16, and `memmap2` 0.9.11, contains no `anyhow`, and passes
its own zero-vulnerability/zero-warning RustSec scan.

The verified auditor then runs `scripts/audit-rust-dependencies.sh` against
SchemaHub's committed `Cargo.lock`. The gate parses the machine report and
requires zero vulnerabilities plus exactly the two reviewed warnings below.
Any new, changed, or disappeared warning fails until the policy and lockfile
are reviewed; vulnerabilities, unsoundness, malformed output, scan failure, or
an auditor-version mismatch also fail. The JWT implementation uses
jsonwebtoken's `aws_lc_rs` backend: the prior RustCrypto `rsa` path is absent
from the lockfile, RS256 JWK verification has a real signed-token test,
`crossbeam-epoch` is locked at 0.9.20, and `anyhow` is locked at 1.0.104.

The same verified auditor runs `scripts/verify-cargo-auditable.sh` against the
exact cargo-auditable 0.7.5 crates.io archive and its published lock before the
release matrix may start. Both the source archive and 48-package lock have
pinned SHA-256 identities, and the complete lock currently passes with zero
vulnerabilities or warnings. Each Linux, macOS, and Windows build then forces a
fresh `--locked` install beneath its isolated runner-temporary root and invokes
that exact `cargo-auditable` binary directly. A later RustSec disclosure,
source/lock drift, ambient preinstalled binary, or PATH collision therefore
fails before release instrumentation.

The audit currently permits two non-vulnerability warnings:

- `paste` 1.0.15 is unmaintained through `utoipa-axum`; there is no patched
  upstream release in the current API stack.
- `spin` 0.9.8 is yanked under `sqlx-sqlite` in the lockfile, but the SQLite
  branch is not in SchemaHub's resolved feature tree.

The released `utoipa-axum` 0.2.0 still depends on `paste`; upstream `master`
has switched to `pastey` together with an Axum 0.8 move. SchemaHub remains on
the released Axum 0.7 integration until that maintained combination is
published and its route/OpenAPI contract passes. SQLx 0.9's supported facade
lists every database driver in its package dependency set, so Cargo retains
the optional SQLite branch even though the all-feature `schemahub-server`
dependency tree does not resolve it. SchemaHub will remove that warning when
the supported SQLx facade stops retaining the yanked edge; it does not replace
the facade with semver-exempt `sqlx-core` internals merely to alter lockfile
diagnostics.

The GUI and workflow demo each run `pnpm audit --audit-level low` after a
frozen install. Workspace overrides lock patched Vite, esbuild, PostCSS,
Next.js, and Sharp versions. The GUI has exactly one advisory exception:
`GHSA-qwww-vcr4-c8h2`, which affects React Router's unstable server-side RSC
APIs. SchemaHub is a client-only Vite application using `BrowserRouter` and
imports no RSC/server surface. `scripts/test-dependency-audit-policy.sh`
enforces both auditor identities and patched tool-lock versions, the SchemaHub
crypto backend and patched lock versions, exact one-item exception, client-only
routing scope, and presence of all audit CI steps.
`scripts/test-install-cargo-audit.sh` proves invalid source overrides and
archive checksum drift fail before installation.
`scripts/test-verify-cargo-auditable.sh` provides the equivalent fail-closed
source and auditor preconditions for the release build tool. Expanding the
exception, restoring a stale tool lock, using an ambient instrumentation
binary, or removing an audit therefore fails before release compilation.

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

## Real-World Scenario Evidence Gate

The redb CI job runs all seven isolated codelabs and then invokes
`scripts/render-ga-readiness-report.sh`. The reporter requires the exact
scenario set, complete process evidence, passing normalized results, and zero
open release-blocker or high findings in
`codelabs/real-world/findings.json`. Candidate runs also fail when the checked
out source is dirty.

CI retains a deterministic `schemahub-ga-readiness.tar.gz` containing the
human and JSON decisions plus the seven normalized result summaries. Each
summary digest is recorded in the report. Because the release workflow reuses
the complete CI gate and downloads all retained artifacts before checksum
assembly, it revalidates the tag SHA, clean provenance, scenario/finding
decision, archive paths, and all seven normalized result digests. The exact
scenario evidence accepted for the tag then becomes a checksummed release
asset.

This gate proves repository scenarios only. Its machine contract always leaves
`release_authorized` false and lists exact-digest staging, real-provider JWT
rotation/staleness, and explicit tag authorization as separate gates.
Lower-severity findings may declare a stable `must_fix_before` deadline. The
release metadata job compares the target version with every open deadline
before compiler checkout or artifact construction. A prerelease at the
deadline remains available for validation, but the corresponding stable
release and every later version fail closed until the finding is marked fixed.

## Stable Staging Promotion Gate

Prerelease tags publish after the complete reusable CI gate so operators have
an immutable candidate to deploy. Stable `vMAJOR.MINOR.PATCH` tags additionally
wait on the protected `schemahub-production-staging` environment after the
multi-architecture image is pushed under the unique
`candidate-<run-id>-<run-attempt>` tag. The semantic-version container tag and
GitHub release are both withheld until staging succeeds.
Before consuming its attestation, the workflow reads both the environment and
its deployment-policy collection. It requires an independent reviewer with
self-review disabled and exactly one `v*.*.*` custom policy; missing, broader,
branch-typed, or additional policies fail closed.

The environment must provide a fresh
`SCHEMAHUB_STAGING_ATTESTATION_B64` value. The versioned attestation is checked
against the exact tag SHA, stable version, image repository and digest, plus
the SHA-256 digest of the GA-readiness archive produced by that same workflow
run. It also asserts PostgreSQL deployment, real-provider key
rotation/staleness/recovery, human/agent acceptance, same-origin bundled-GUI
serving, restart and prior-candidate byte identity, corruption rejection,
visible/hidden reverse discovery, and backup/restore. Exact object keys, HTTPS
evidence coordinates, digest shapes, secret-like content, and seven-day
freshness fail closed.

After validation the workflow retains the normalized
`schemahub-staging-attestation.json`. Stable publication requires the staging
job to succeed; prerelease publication explicitly tolerates its skipped state.
The workflow next verifies the retained GA report, renders the final notes,
builds the distribution SBOM and `SHA256SUMS`, and retains that complete
assembly without publishing it. An exact-set verifier rejects subdirectories,
unsafe or duplicate names, missing files, unchecksummed extras, malformed
entries, and checksum mismatches before upload. The workflow retains GitHub's
artifact SHA-256 output, then repeats the exact-set and checksum verification
after download. Only then does an idempotent promotion job create the
semantic-version tag from the already-attested digest. It refuses to overwrite
that tag when the registry already resolves it to any other digest and verifies
the final tag before the GitHub release can consume the reverified assembly.
The attestation is published as a checksummed release asset. Setup and operator
commands are in `codelab-stable-release-staging.md`.

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
- `schemahub-staging-attestation.json` for stable releases
- `RELEASE-NOTES.md`
- `SHA256SUMS`
- `ghcr.io/shuozeli/schemahub:X.Y.Z` for `linux/amd64` and `linux/arm64`
- a run-unique pre-promotion tag
  `candidate-<run-id>-<run-attempt>` plus BuildKit provenance

Archives contain the server, CLI, README, compatibility policy, the
release-binary-generated `schemahub-http-openapi.json`, the exact locked
`schemahub-gui/` production bundle, and exact SchemaHub/compiler revisions.
Release and container binaries are built with `cargo-auditable`; the image SBOM
must enumerate Rust crates as well as base OS packages.

Native archives are byte-reproducible for identical platform inputs. The
OpenAPI HTTP route and CLI share one recursively key-sorted JSON byte sequence.
Packaging sorts member paths, rejects newline-containing paths, writes ustar
with UID/GID `0/0`, normalizes ordinary files to `0644` and binaries to `0755`,
sets every member timestamp to `2000-01-01T00:00:00Z`, and compresses with
no-name gzip metadata. Each Linux, macOS, and Windows release job packages the
same inputs into two separate directories and requires `cmp` equality before
uploading either archive.

The tag must have a version-matched source document under `docs/releases/`.
The release workflow validates its upgrade, migration, mixed-version, rollback,
compatibility, known-issue, and provenance sections; injects the exact source,
compiler, and multi-architecture image coordinates; includes the rendered notes
in the distribution SBOM and checksums; and uses that same file as the GitHub
release body. Missing notes, unresolved template markers, or malformed
coordinates fail before publication.

The `1.0.0` contract additionally freezes `schemahub.v1` versus the GUI BFF,
states the OpenAPI-codegen, GUI-authoring, repository-search, and
cross-repository coordination limits, and requires the stable staging evidence
section. CI exercises positive rendering plus missing-section, missing-boundary,
and unresolved-marker failures.

The container is distroless, runs as UID/GID `65532`, includes both binaries,
the exact production GUI, and the PostgreSQL backend. Its default redb data
directory is `/var/lib/schemahub`; gRPC and HTTP listen inside the container on
ports 50051 and 8080, and the HTTP listener serves
`/usr/share/schemahub/gui` at `/`. Runtime validation refuses symbolic links or
non-file/directory entries anywhere in a configured GUI tree, matching the
release packaging boundary and preventing static-file root escape. Only
Vite-style content-hashed asset filenames receive immutable caching; successful
unhashed assets receive `no-cache`. Host port mappings must bind to the
Tailscale interface. CI creates a real named volume,
writes a schema through an
authenticated non-root process, materializes descriptor and generated-Rust
artifacts, removes that process, starts a replacement against only the retained
volume, and verifies the exact schema revision and both artifact digests.

All three external Dockerfile stages name exact multi-architecture manifest
digests: Node 24 Bookworm Slim for the GUI, Rust 1.95.0 Bookworm for the
builder, and `cc-debian12:nonroot` for runtime. The pins cover both release
platforms (`linux/amd64` and `linux/arm64`). The Dockerfile frontend, PostgreSQL
17 integration service, and curl acceptance helper are also fixed to exact
multi-architecture manifests, while the image's pnpm 11.2.2 and
cargo-auditable 0.7.5 coordinates cannot be overridden. Workflow GUI builds
use the exact Node 24.18.0 runtime. The tag workflow cannot silently pick up a
moved tag or Node patch, and
`scripts/test-container-supply-chain-policy.sh` rejects drift in any of these
coordinates or an external GitHub action that is not fixed to a 40-character
commit.

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
version, build and budget-check the GUI, and package both:

```bash
(
  cd apps/schemahub-gui
  pnpm install --frozen-lockfile
  pnpm run build
  pnpm run test:bundle
)
export SCHEMAHUB_VERSION=0.9.0-rc.1
export SCHEMAHUB_REVISION="$(git rev-parse HEAD)"
export PROTOBUF_RS_REVISION="$(scripts/validate-compiler-lock.sh protobuf --development)"
export FLATBUFFERS_RS_REVISION="$(scripts/validate-compiler-lock.sh flatbuffers --development)"
audit_root="$(mktemp -d /tmp/schemahub-cargo-audit-install.XXXXXX)"
SCHEMAHUB_CARGO_AUDIT_INSTALL_ROOT="$audit_root" \
  scripts/install-cargo-audit.sh
SCHEMAHUB_CARGO_AUDIT_BIN="$audit_root/bin/cargo-audit" \
  scripts/verify-cargo-auditable.sh
auditable_root="$(mktemp -d /tmp/schemahub-cargo-auditable-install.XXXXXX)"
cargo install cargo-auditable \
  --locked \
  --version 0.7.5 \
  --force \
  --root "$auditable_root"
"$auditable_root/bin/cargo-auditable" auditable build --release --locked \
  --target x86_64-unknown-linux-gnu \
  -p schemahub-server -p schemahub-cli \
  --features schemahub-server/postgres
scripts/package-release.sh \
  "$SCHEMAHUB_VERSION" x86_64-unknown-linux-gnu target/release-check
```

The packaging script refuses missing binaries, a `--version` mismatch, an empty
generated OpenAPI document, mutable or malformed release versions, and
missing/non-SHA SchemaHub or compiler provenance. It never emits an archive
containing `unknown` revisions or a path that cannot be represented by its
deterministic newline-delimited member list.

On 2026-07-25, the current source passed this exact host-target rehearsal for
`0.9.0-rc.1`. Both packaged binaries contained cargo-auditable `.dep-v0`
metadata and reported the requested version; the embedded OpenAPI reported
`0.9.0-rc.1` with 22 paths and 24 operations; and `BUILD-METADATA.txt` named
the exact SchemaHub, Protobuf, and current FlatBuffers revisions. The archive
digest was
`sha256:673e5d6f39485df96aecb17003ba8d9d3442ea7697f1b59bfbd61caf772f1b49`.
The source worktree was dirty and the warning-clean FlatBuffers follow-up was
not the pinned dependency, so this proves packaging behavior only.

On 2026-07-29, `RV-RELEASE-003` closed the remaining byte-reproducibility gap.
Eight fresh server processes emitted bytes identical to HTTP discovery, and
two real auditable host-target packages assembled in separate directories were
byte-for-byte equal at
`sha256:9291e80bf47cdc44de06090fe43f45d54bf33d22e512481457558e154cee74eb`.
Their embedded OpenAPI digest was
`sha256:ef8876a87dc66a8be2839d5700b639f203a7a247dde9c0c058fc7a87dc025026`;
repeated execution of the extracted server reproduced it exactly. Archive
inspection confirmed sorted files, UID/GID `0/0`, fixed UTC timestamps, and
normalized modes. Strict all-target/all-feature workspace Clippy, all 609
default-workspace release tests, and the separate 25-test PostgreSQL release
slice also passed. This remains dirty-worktree rehearsal evidence, not
publishable provenance.

The same source built through the hermetic Rust 1.95 Docker path with the
PostgreSQL feature and immutable provenance labels. Local image
`schemahub:local-goal-20260725`
(`sha256:07513bdae4b1dc7ed59a34e84fa6d1a286d9a61983f6449aae7d017a454e6279`,
25,025,337 bytes) ran as UID/GID `65532`, became healthy, served readiness and
build metrics, returned the compiler capability matrix, reported the requested
version from both binaries, and exited 0 after `SIGTERM`. This current runtime
rehearsal likewise does not replace a clean candidate Actions run. Syft 1.48
generated a valid SPDX 2.3 document with 457 records: 446 Cargo packages, 10
Debian packages, and the OCI image. It includes every SchemaHub workspace crate
and both compiler stacks; the local document digest is
`sha256:eb4c6ed3786f1f5404bc5c76861b8a7aa136e1510b589c23337cc56bea76122b`.

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

On 2026-07-29, the hardened dirty worktree rebuilt through that same hermetic
Rust 1.95/PostgreSQL/distroless path after the JWT backend moved to AWS-LC.
Local image `schemahub:security-ci`
(`sha256:6945243c617d971294b475e88097c668570653a88e7f5ba811dedb95baa6ad07`,
25,091,631 bytes) labels the exact Protobuf and warning-clean FlatBuffers
coordinates, runs as UID/GID `65532`, and reports `0.0.0-security` from both
binaries. The runtime acceptance served the bundled GUI and immutable assets,
kept unknown BFF routes non-HTML, created schema state, materialized descriptor
and generated Rust artifacts, drained cleanly, replaced the container over the
same named volume, and verified both artifacts byte-for-byte. Their digests
were
`sha256:25a7372f253d6a6b4d93d22eb78dd22943aecacca8dfd49f2ae5fc1ced7b5e9e`
and
`sha256:c507375f3602e3c2bc0aecc761ef8cfb9b5e25a1b59fedae29d517f90e1e47c3`.
This proves the new crypto graph in the production build/runtime path, not
clean candidate provenance or real-provider staging.

The subsequent exact-base rebuild, including the fail-closed GUI filesystem
and content-hash cache boundaries, produced local image
`sha256:380c649e600fa39a90d43a9846fcc87d502ddbf51d3946ba4cb37a77fc6a69d7`.
It runs as `65532:65532`, embeds cargo-auditable metadata and both exact compiler
revisions, accepted the shipped symlink-free GUI, and passed the complete
named-volume process-replacement acceptance with descriptor and generated-code
digests unchanged. This remains dirty-worktree rehearsal evidence.

After closing `RV-HTTP-002`, the exact-base production rebuild produced local
image
`sha256:79d333b36b10bdee47641062d54e09223b11f326231ed56188e85be1574d4e78`.
The container acceptance asserted the complete CSP, framing denial, restricted
camera/geolocation/microphone permissions, MIME protection, referrer policy,
HTML `no-cache`, and hashed-asset immutable caching from the distroless image.
It again replaced the non-root container over the same named redb volume and
returned descriptor digest
`sha256:25a7372f253d6a6b4d93d22eb78dd22943aecacca8dfd49f2ae5fc1ced7b5e9e`
and generated-code digest
`sha256:c507375f3602e3c2bc0aecc761ef8cfb9b5e25a1b59fedae29d517f90e1e47c3`.
Pwright then rendered that image from the Tailscale-bound host through the
neutral ARM2 CDP endpoint, reporting the expected `SchemaHub Console` title
and Projects UI. This is still dirty-worktree rehearsal evidence, not candidate
provenance.

After the bounded active/all project and per-project repository catalog fix,
bounded project-membership and branch/tag responses, bounded
project/repository GUI selectors, and bounded batch-loaded dashboard and
ChangeRecord aggregates, the subsequent fresh-fixture seven-scenario portfolio
also passed with zero open findings. Its normalized report digest is
`sha256:20b4575efdf4a95411d6213e573949da80bbd7dc7da536076c971cf1b12761b4`
and its CI-equivalent deterministic archive digest is
`sha256:93c203bfaff0491f72fe12dd8964729309d33b7d1ad9015eb1763490afc83aba`.
Two independently packaged archives were byte-identical. The same source
passed 699 release-workspace tests, strict all-target/all-feature Clippy, 28
PostgreSQL integration tests, the GUI production/bundle/CDP resolver gates,
the remote mock continuation smoke, and the live
agent-author/human-review/agent-Apply/restart browser acceptance. A
neutral-Pwright run independently exercised the dashboard continuation with
real CDP input.
The reporter correctly withheld release authorization, and the verifier
rejected the archive, because the source is still a dirty, unpushed worktree.

## Candidate and Stable Gates

Before creating a 0.9 prerelease tag:

1. Publish and pin clean compiler dependency commits.
2. Require the locked Rust and both frozen pnpm dependency audits plus the
   dependency-policy contract to pass without an unreviewed exception.
3. Require every CI job on the candidate commit, including the executable
   `e2e_ga_acceptance` journey. It must preserve delegated-agent authorship,
   human review, and exact Protobuf/FlatBuffers artifact bytes across redb
   restart.
   Inspect the retained `schemahub-ga-readiness.tar.gz`: it must name the exact
   candidate SHA, report clean provenance, show seven passing scenarios, and
   contain zero open release-blocker/high findings.
4. Verify the redb and PostgreSQL backup/restore drills.
5. Generate and inspect both SBOMs and `SHA256SUMS`.
6. Verify a native archive contains `schemahub-gui/index.html`, and verify the
   exact image serves `/`, a nested `/projects/...` route, and its hashed entry
   asset with the required browser security headers, without a remote CDN
   request or converting unknown `/api/*` paths to HTML.
7. Obtain explicit authorization before pushing the prerelease tag.

The prerelease produces the candidate image needed for staging. Before
promoting it to a stable release:

1. Push the explicitly authorized stable tag and wait for its container job to
   produce the exact candidate digest. The semantic-version image tag remains
   absent while staging is pending.
2. Deploy that exact digest to production-like PostgreSQL staging.
3. Verify the exact image's same-origin GUI root, direct nested route, CSP,
   framing and browser-feature restrictions, hashed-asset caching, absence of
   remote application-asset requests, and non-HTML unknown-BFF `404`.
4. Complete the human/agent change-to-artifact workflow, restart, and compare
   exact bytes.
5. Verify an artifact first served by the prior candidate is returned
   byte-for-byte by the new candidate with the same persistent database, and
   confirm a corrupt fixture fails closed instead of rerendering.
6. Complete the real-provider JWT rotation/staleness drill in staging; static
   tokens do not satisfy this gate.
7. Exercise `ListDependents` with live, pinned, and unreadable downstream
   repositories; retain the snapshot manifest with the acceptance evidence.
8. Restore the PostgreSQL backup into a fresh database and verify the retained
   immutable artifacts.
9. Build and locally validate the exact staging attestation, set it on the
   protected environment, and obtain independent environment approval.
10. Confirm the release notes, distribution SBOM, and `SHA256SUMS` assemble
    successfully before any semantic-version image tag is created.
11. Confirm the promotion job maps the semantic-version image tag to exactly
    the attested digest without overwriting any different existing tag.
12. Review `CHANGELOG.md`, migration policy, compatibility policy, known
   limitations, and the generated BFF/operations path classifications.

Release notes must state the source and target versions, migration set,
mixed-version allowance, rollback window, compatibility changes, known issues,
and exact container digest. `scripts/render-release-notes.sh` enforces this
contract and the tag workflow publishes only its validated output.
