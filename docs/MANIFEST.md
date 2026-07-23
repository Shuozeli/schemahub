<!-- agent-updated: 2026-07-22T15:54:03Z -->
# Documentation Manifest

| File | Covers | Update When |
|---|---|---|
| `../README.md` | Product overview, setup, CLI, configuration, and current limitations | User-facing features, commands, configuration, ports, or limitations change |
| `../CHANGELOG.md` | Unreleased and published user-facing changes plus release blockers | A user-visible change lands, a release is cut, or blocker status changes |
| `product.md` | Product purpose, actors, resources, workflows, principles, and 1.0 success criteria | Product scope, core resource semantics, or release success criteria change |
| `roadmap.md` | Ordered deliverables and acceptance gates through 1.0 | A phase is added, removed, reordered, completed, or materially rescoped |
| `tasks.md` | Pending execution checklist and milestone status | Work completes, a blocker appears, or a deliverable gains or loses tasks |
| `change-records.md` | Change resource, lifecycle, API, validation, and crash-recovery design | Change workflow, storage protocol, actor model, or API contract changes |
| `serving.md` | Immutable revisions, first-materialization persistence, artifact kinds, digest encoding, cache semantics, and CLI | Serving resources, persistence, digest format, artifacts, cache behavior, or fetch/verify commands change |
| `resources-and-policy.md` | Durable project/repository resources, atomicity, policy, archive behavior, and JSON migration | Project/repository lifecycle, persistence, migration, or policy behavior changes |
| `authentication.md` | Noop/static/JWT modes, claims, JWKS refresh, fail-closed readiness, and rotation operations | Authentication mode, JWT validation, claims, key loading/refresh, readiness, or identity mapping changes |
| `idempotency.md` | Direct-write receipt scope, fingerprints, leases, JJ correlation, retention, cleanup, and errors | Write surfaces, receipt protocol, bounds, recovery, or status mapping changes |
| `dependency-discovery.md` | Immutable forward closures plus bounded visible-repository reverse scans, pins, unresolved edges, snapshot manifests, limits, and coordination semantics | Import discovery, resolution, scan bounds, authorization, snapshots, or cross-repository coordination changes |
| `requirements.md` | Functional and non-functional requirements | Required behavior or fixed technology constraints change |
| `design.md` | Compiler/JJ architecture, mutation, compatibility, auth, and codegen design | Core architecture, persistence model, or major flow changes |
| `crate-structure.md` | Workspace crates, dependency graph, and ownership boundaries | Crates, dependencies, modules, or ownership boundaries change |
| `grpc-api.md` | gRPC resources, methods, semantics, authentication, and errors | Protobuf contracts or RPC behavior change |
| `http-api.md` | Handler-generated OpenAPI contract, discovery, auth metadata, catch-all paths, packaging, and REST/BFF boundary | HTTP routes, DTOs, OpenAPI generation, release packaging, or REST compatibility scope changes |
| `format-capabilities.md` | Versioned format features, mutation reachability, import pins, and reference-integrity contract | A compiler workflow, advertised operation, codegen language, or capability RPC changes |
| `openapi-ast.md` | OpenAPI AST, blob encoding, parsing, printing, and operations | OpenAPI schema representation or compiler behavior changes |
| `ui-design.md` | Web-console product, component, and generated BFF-contract design | Navigation, workflows, screens, BFF contract, or component architecture change |
| `gui.md` | Implemented GUI architecture, routes, generated HTTP contract, boundary policy, setup, and limitations | GUI implementation, dependencies, BFF routes/policy, OpenAPI discovery, or setup changes |
| `codelab-human-agent-schema-workflow.md` | Primary delegated-agent proposal, human review, Apply, immutable artifact, and data-schema-coordinate tutorial | ChangeRecord lifecycle, actor/reviewer roles, CLI JSON, repository policy, serving commands, or artifact identity changes |
| `codelab-cli-grpc.md` | End-to-end CLI/gRPC tutorial | CLI commands, RPC workflow, generated output, or setup changes |
| `codelab-operations.md` | Health, logs, metrics, OpenAPI discovery, migrations, backup/restore, upgrade/rollback, and GC drills | Deployment behavior, probes, generated contract, telemetry, persistence, migration, or recovery policy changes |
| `codelab-deploy.md` | Tailscale-safe release-container deployment, HTTP/OpenAPI/auth policy, and change-to-artifact acceptance rehearsal | Image/runtime behavior, deployment commands, HTTP/OpenAPI/auth setup, or release acceptance changes |
| `release.md` | CI matrix, compiler coordination, binaries/OpenAPI artifacts, SBOMs, tag process, and candidate gates | CI/release workflow, artifact platform, generated contract, dependency revision, or publication policy changes |
| `releases/0.9.0-rc.1.md` | Version-specific upgrade contract, compatibility changes, known issues, and immutable release provenance | The 0.9 candidate scope, migration/mixed-version/rollback policy, known issues, or release coordinates change |
| `compatibility-policy.md` | 0.x/1.x API, storage, artifact, CLI, and operations compatibility promises | A public stability promise, deprecation rule, migration policy, or freeze blocker changes |
| `code-quality-findings.md` | Historical code-quality audit and known issues | A listed issue is fixed, invalidated, or newly discovered |
| `ADR/0001-change-records-and-serving-plane.md` | Decision to separate durable change intent from immutable serving | Never; supersede with a new ADR |
| `ADR/0002-public-api-and-gui-bff-boundary.md` | Decision to make `schemahub.v1` public and keep `/api/*` a GUI-only BFF | Never; supersede with a new ADR |
