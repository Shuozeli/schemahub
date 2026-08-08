<!-- agent-updated: 2026-07-30T04:16:42Z -->
# Authentication and Production Identity

SchemaHub is an OAuth 2/OIDC-compatible resource server: clients obtain bearer
JWTs from an external identity provider, while SchemaHub verifies those tokens
and applies its own durable project roles. SchemaHub does not issue tokens,
exchange authorization codes, host a login page, or derive authorization from
OAuth scopes.

## Modes

Exactly one credential mode is active at startup:

| Mode | Configuration | Intended use |
|---|---|---|
| `noop` | No tokens, JWT block, or project bootstrap | Local evaluation only; no authorization enforcement |
| `static-bearer-rbac` | `[auth].tokens` and/or `[projects.*]` | Development and trusted-tailnet rehearsals |
| `jwt-rbac` | `[auth.jwt]` | Production external identity plus durable SchemaHub RBAC |

Static tokens and `[auth.jwt]` are mutually exclusive. A mixed configuration
fails startup. Missing credentials still resolve to anonymous so public project
reads remain possible. A presented but malformed, expired, or unverifiable JWT
returns `UNAUTHENTICATED`/HTTP `401`; it never falls back to anonymous.

## Production Configuration

Every field inside `[auth.jwt]` is required. This keeps trust, time, size, and
availability choices explicit:

```toml
[auth.jwt]
issuer = "https://identity.example.com"
audiences = ["schemahub"]
algorithms = ["RS256"]
token_type = "at+jwt"
identity_id_prefix = "corp-oidc:"
jwks_url = "https://identity.example.com/.well-known/jwks.json"
clock_skew_seconds = 30
refresh_interval_seconds = 300
max_stale_seconds = 1800
request_timeout_seconds = 5
max_token_bytes = 8192
max_jwks_bytes = 1048576

[projects.payments]
visibility = "private"
owners = ["corp-oidc:248289761001"]
members = { "corp-oidc:schema-agent" = "Writer" }
```

Configure exactly one key source:

- `jwks_url` must be an absolute HTTPS URL without credentials, query, or
  fragment. Redirects are disabled. The URL is operator-configured; JWT header
  values never influence network requests.
- `jwks_file` loads a local public JWKS for air-gapped deployments and tests.
  Replace it atomically, then allow the refresh interval to reload it.

Remote and file responses are bounded by `max_jwks_bytes`. The initial JWKS
must load and contain at least one usable configured signing key or startup
fails. `max_stale_seconds` must be at least twice `refresh_interval_seconds`.

Only asymmetric JWT algorithms are accepted. `HS256`, `HS384`, and `HS512`
are rejected at configuration time so a public verification key can never be
confused with a shared HMAC secret. Keep the algorithm list to the smallest set
the issuer actually uses.

Production signature verification uses jsonwebtoken's AWS-LC backend rather
than its RustCrypto RSA backend. The locked graph contains no `rsa` crate, and
an RS256 token signed for a real RSA JWK is exercised alongside the EdDSA,
claim-validation, rotation, and stale-key tests. Release CI runs pinned
RustSec auditing with a zero-vulnerability and exact reviewed-warning contract,
so a crypto-backend regression or new warning cannot enter a candidate only
through lockfile drift.

## Token Contract

The protected JWT header must contain:

- `alg`: a configured asymmetric algorithm;
- `kid`: a non-empty identifier matching one usable JWKS signing key;
- `typ`: an exact match for configured `token_type`, commonly `at+jwt` or
  `JWT`.

SchemaHub rejects critical headers it does not implement, nested/encrypted
tokens, and token-supplied `jku`, `jwk`, `x5u`, or `x5c` key material. A JWKS
key must be marked for signature verification when `use` or `key_ops` is
present, declare its algorithm, match that algorithm's key family, and have a
unique `kid`.

Required claims:

| Claim | Meaning |
|---|---|
| `iss` | Exact configured issuer |
| `aud` | String or array containing one configured audience |
| `sub` | Non-empty issuer subject used as the principal |
| `exp` | Expiration as Unix seconds |

Optional claims:

| Claim | Behavior |
|---|---|
| `nbf` | Token is rejected before this time, subject to configured skew |
| `iat` | A future issue time is rejected, subject to configured skew |
| `name` | Audit/UI display name; never used for authorization |
| `schemahub_identity_kind` | `human`, `agent`, or `service`; absent means `human` |
| `schemahub_delegated_by` | Delegating issuer subject; valid only for an `agent` |

The durable SchemaHub identity is `identity_id_prefix + sub`. Agent delegation
is normalized with the same prefix. This makes the IDs in `[projects.*]` and
member RPCs stable and prevents subjects from different issuer namespaces from
colliding when operators give each issuer a distinct prefix. Principal kind and
display claims are audit metadata, not privilege; project roles remain the only
authorization input.

Example trusted claims for an agent run:

```json
{
  "iss": "https://identity.example.com",
  "aud": "schemahub",
  "sub": "schema-agent",
  "exp": 1784640000,
  "name": "Schema Maintenance Agent",
  "schemahub_identity_kind": "agent",
  "schemahub_delegated_by": "248289761001"
}
```

## Key Refresh and Fail-Closed Behavior

The server fetches or reads keys before opening for traffic. A supervised task
then refreshes them at `refresh_interval_seconds`:

1. Parse and validate the complete replacement away from the request path.
2. Atomically swap it into the synchronous verifier only when valid.
3. Retain the last known-good set after a failed or malformed refresh.
4. Reject every presented JWT once the last successful refresh is older than
   `max_stale_seconds`.

The HTTP `/readyz` response reports `authentication.status = "stale_keys"` and
returns `503` when that bound is exceeded. Structured events are emitted as
`schemahub.auth.jwks_loaded`, `schemahub.auth.jwks_refreshed`,
`schemahub.auth.jwks_refresh_failed`, and
`schemahub.auth.jwks_freshness_changed`. Tokens and schema source are never
logged. The readiness counter labels this outcome `auth_failure`.

## Rotation Drill

Run this against staging before accepting an identity-provider change:

1. Publish a JWKS containing both the current and next public keys with unique
   `kid` values.
2. Wait for `schemahub.auth.jwks_refreshed` and confirm `/readyz` is `200`.
3. Issue a short-lived test token under the next key and complete a read plus a
   ChangeRecord write authorized by its durable project role.
4. Switch normal issuance to the next key.
5. After every token signed by the prior key has expired, remove that key and
   observe another successful refresh.
6. In an isolated staging instance, make the JWKS unavailable beyond
   `max_stale_seconds`; verify `/readyz` becomes `503` and a formerly valid JWT
   fails closed. Restore the endpoint and verify readiness recovers after a
   successful refresh.

JWT verification does not provide per-token revocation. Use short token
lifetimes, issuer-side session/revocation controls, and emergency signing-key
rotation according to the identity provider's policy. Removing a key revokes
every still-live token signed by it.

## Deployment Notes

- Start production instances with `--config /explicit/path/schemahub.toml`.
  Explicit paths are required: missing, unreadable, or malformed files abort
  startup instead of falling back to anonymous defaults. Only an omitted flag
  permits a missing local `./schemahub.toml` for development.
- Mount a local JWKS as read-only when using `jwks_file`; it contains public
  keys but remains part of the trusted deployment configuration.
- Ensure the runtime has a valid CA trust store and egress only to the explicit
  `jwks_url` host when using remote keys.
- Use a full Tailscale MagicDNS hostname for SchemaHub client URLs. The JWKS
  endpoint itself must be trusted HTTPS.
- The GUI and CLI currently accept a caller-supplied access token; interactive
  browser authorization-code/PKCE login is outside the 1.0 server contract.

The validation policy follows
[JWT Best Current Practices (RFC 8725)](https://www.rfc-editor.org/rfc/rfc8725.html):
explicit algorithms, issuer and audience validation, no trust in received key
claims, and distinct rules for the configured token type. Operators configure
the trusted JWKS URL directly; automatic OpenID Provider discovery is not part
of the current contract.
