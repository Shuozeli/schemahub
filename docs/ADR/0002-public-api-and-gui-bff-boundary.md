<!-- agent-updated: 2026-07-21T16:49:05Z -->
# ADR 0002: Keep the GUI BFF Outside the Public API Promise

## Status

Accepted on 2026-07-21.

## Context

SchemaHub has two network-facing application transports. The
`schemahub.v1` protobuf package exposes resource-oriented gRPC services used by
the CLI, agents, automation, and future generated clients. The optional HTTP
listener also exposes unversioned `/api/*` JSON routes tailored to the bundled
React console.

The JSON DTOs combine and reshape Core resources for individual screens. They
are not HTTP transcoding of every public RPC, do not use a versioned path, and
do not consistently follow the project's Google AIP resource shapes. Treating
them as public REST v1 would freeze a second, incomplete API and couple future
GUI work to a compatibility promise that was not designed for general clients.

The same listener also exposes `/healthz`, `/readyz`, and `/metrics`. Those are
operational interfaces with their own compatibility policy, not GUI routes.

## Decision

1. The `schemahub.v1` gRPC/protobuf contract is SchemaHub's designated public
   1.0 API. The CLI remains a supported client of that contract.
2. Every unversioned `/api/*` route is a GUI-only backend-for-frontend (BFF).
   It is documented and authenticated, but excluded from the 1.x public API
   semantic-versioning promise. The bundled GUI and BFF are supported as a
   same-release pair.
3. Every `/api/*` response carries
   `x-schemahub-api-surface: gui-bff`, including error and discovery responses.
   `/healthz`, `/readyz`, and `/metrics` do not carry that BFF header.
4. The generated OpenAPI document labels each path with
   `x-schemahub-api-surface` and `x-schemahub-compatibility-promise`. Its info
   metadata names `schemahub.v1` as the public API and `/api/` as the BFF
   prefix. `info.version` identifies the exact server build; it is not a REST
   API stability version.
5. BFF changes still preserve authentication, authorization, audit,
   idempotency, publication, and immutable-serving invariants. They are
   recorded in release notes and the generated contract, even when they are
   allowed to change with the bundled GUI.
6. A future public REST API must use an explicit versioned prefix such as
   `/v1/`, follow the accepted resource and method conventions, publish a
   separately identified OpenAPI contract, and receive an explicit
   compatibility declaration. It does not silently inherit the `/api/*`
   routes.

## Consequences

- Humans and agents that need a durable integration use generated gRPC clients
  or the CLI rather than binding automation to browser DTOs.
- Operators deploy matching GUI and server versions. Cross-version BFF/GUI
  compatibility is best effort rather than a 1.x guarantee.
- The generated HTTP document remains valuable for exact-build discovery,
  browser client generation, testing, and security review without implying a
  public REST promise.
- Operational probes and metrics remain supported according to
  `compatibility-policy.md`, independent of the BFF classification.
- Adding public REST later is an additive, separately reviewed project instead
  of an accidental promotion of convenience routes.

## Verification

Release-mode HTTP integration tests assert the response-header boundary, the
absence of the BFF marker on operational routes, the per-path OpenAPI
classification, the public `schemahub.v1` metadata, and CORS exposure of the
classification header.
