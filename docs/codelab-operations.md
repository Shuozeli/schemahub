<!-- agent-updated: 2026-07-21T16:49:05Z -->
# SchemaHub Operations Codelab

This runbook covers the production mechanics needed to keep schema revisions
available to data producers and consumers: startup probes, structured logs,
metrics, graceful shutdown, database migrations, backup/restore, upgrade, and
rollback, including production JWT verification-key rotation. Run destructive
restore steps against a new database or file path; never overwrite the only
known-good copy.

## 1. Build the release artifact

The canonical tag build uses `cargo-auditable`, publishes checksums and SPDX
SBOMs, and injects one version into every binary/probe/metric/OCI surface. See
`docs/release.md` for the full matrix. For a local operational build, redb is
the embedded default:

```bash
CARGO_INCREMENTAL=0 cargo build --release -p schemahub-server
```

PostgreSQL support is feature-gated:

```bash
CARGO_INCREMENTAL=0 cargo build --release -p schemahub-server --features postgres
```

Verify that the exact binary can generate its HTTP contract without startup:

```bash
target/release/schemahub-server --print-openapi \
  | jq -e '.openapi == "3.1.0"
    and (.paths | length > 0)
    and .info["x-schemahub-public-api"] == "schemahub.v1"
    and .paths["/api/projects"]["x-schemahub-api-surface"] == "gui-bff"
    and .paths["/healthz"]["x-schemahub-api-surface"] == "operations"'
```

`schemahub-api` uses a vendored platform-specific `protoc`; these builds do not
depend on a system `protobuf-compiler` package. The deployment rehearsal using
the non-root distroless image is in `docs/codelab-deploy.md`.

The process defaults to newline-delimited JSON logs. For an interactive local
run, pass `--log-format pretty`. `RUST_LOG` controls filtering; the default is
`schemahub_server=info,schemahub_core=info,tower_http=info`.

When `--config PATH` is supplied, the path is required and any missing,
unreadable, or malformed file aborts startup. Without the flag, the server
loads `./schemahub.toml` when present and otherwise uses development defaults.
Production deployments should always pass the explicit config path.

## 2. Start on the Tailscale interface

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"

target/release/schemahub-server \
  --listen "$TAILSCALE_IP:50051" \
  --http-listen "$TAILSCALE_IP:8080" \
  --shutdown-timeout-seconds 30 \
  --config schemahub.toml
```

Use the full MagicDNS name from clients:

```bash
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/healthz"
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/readyz"
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/api/openapi.json" \
  | jq -e '.info.version != "" and .info["x-schemahub-public-api"] == "schemahub.v1"'
curl --fail --silent --show-error --dump-header - --output /dev/null \
  "http://$TAILSCALE_HOST:8080/api/openapi.json" \
  | tr -d '\r' | grep -Fx 'x-schemahub-api-surface: gui-bff'
```

`/healthz` proves that the process and HTTP executor are alive. `/readyz`
returns `200` only while the process accepts traffic and a read-only ObjectDb
transaction succeeds and production authentication keys remain within their
configured freshness window. It returns `503` before graceful draining starts,
when storage is unavailable, or with
`authentication.status = "stale_keys"` after JWT refresh has exceeded
`max_stale_seconds`. Both endpoints are intentionally unauthenticated for
orchestrators and send `Cache-Control: no-store`.

The distroless container health check runs the server binary in probe mode:

```bash
schemahub-server --check-ready http://127.0.0.1:8080/readyz
```

The probe has a one-second connection timeout, a two-second total timeout,
does not follow redirects, and exits non-zero for any non-2xx response. The
loopback address is internal to the container; operators and clients should
continue to use the full MagicDNS hostname shown above.

The gRPC listener also implements the standard `grpc.health.v1.Health`
service. Query the empty service name for overall status. It changes from
`SERVING` to `NOT_SERVING` before listener shutdown.

## 3. Collect logs and metrics

Every HTTP request receives an `x-request-id`. A caller-supplied value is
propagated; otherwise SchemaHub generates a UUID. gRPC spans honor the same
header when supplied. ChangeRecord lifecycle events include the change resource
name, project/repository, state, authenticated actor kind, delegation, request
ID, ETag, and immutable Apply receipt IDs. Tokens and schema source are never
logged.

Scrape Prometheus text from:

```bash
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/metrics"
```

The initial metric contract includes:

- `schemahub_build_info`
- `schemahub_http_requests_total`
- `schemahub_http_requests_in_flight`
- `schemahub_http_requests_cancelled_total`
- `schemahub_http_responses_total{class=...}`
- `schemahub_http_request_duration_seconds`
- `schemahub_grpc_requests_total`
- `schemahub_readiness_checks_total{result=...}`

Treat metric names as an operational compatibility surface. Counters are
process-local and reset after restart; durable audit history remains in JJ and
ChangeRecords.

## 4. Gracefully stop or restart

Send `SIGTERM` from a service manager or `SIGINT` interactively. SchemaHub:

1. Publishes HTTP not-ready and gRPC `NOT_SERVING`.
2. Stops accepting new work on both listeners.
3. Drains active requests for `--shutdown-timeout-seconds` (or
   `SCHEMAHUB_SHUTDOWN_TIMEOUT_SECONDS`).
4. Exits `0` after a clean drain, or non-zero if a listener fails or the grace
   period expires.

Do not use `SIGKILL` for routine rollout; it bypasses draining. ChangeRecord
Apply remains retry-safe after a crash because its request ID, attempt lease,
JJ operation attributes, and receipt are durable.

## 5. PostgreSQL migrations

The server embeds checksum-verified SQLx migrations from
`crates/schemahub-jj/migrations/` and applies them before it becomes ready.
Applied versions live in `_sqlx_migrations`. The baseline migration is safe for
databases created by older SchemaHub builds: all initial objects use
`IF NOT EXISTS`, so enrollment records the version without rewriting content.

Inspect migration state:

```bash
psql "$SCHEMAHUB_DATABASE_URL" \
  -c 'SELECT version, description, installed_on, success FROM _sqlx_migrations ORDER BY version;'
```

Migration policy for later releases:

1. Use expand/migrate/contract ordering.
2. Keep release N able to read the expanded schema created for N+1.
3. Backfill independently when a rewrite could exceed the startup budget.
4. Remove old columns or encodings only after the rollback window closes.
5. Never edit an applied migration; append a new one. Checksum drift fails
   startup intentionally.

For least privilege, run the new binary once with a migration-capable role,
then run steady-state instances with only the documented table privileges.

## 6. Back up and restore redb

Redb backup is an offline file snapshot. First drain every SchemaHub process
using the file. Then copy to a new path and record a checksum:

```bash
export SCHEMAHUB_REDB=/var/lib/schemahub/schemahub.redb
export SCHEMAHUB_BACKUP_DIR=/var/backups/schemahub
install -d -m 0700 "$SCHEMAHUB_BACKUP_DIR"
cp --reflink=auto --preserve=mode,timestamps \
  "$SCHEMAHUB_REDB" "$SCHEMAHUB_BACKUP_DIR/schemahub-$(date -u +%Y%m%dT%H%M%SZ).redb"
sha256sum "$SCHEMAHUB_BACKUP_DIR"/*.redb > "$SCHEMAHUB_BACKUP_DIR/SHA256SUMS"
```

Restore to a separate path, verify its checksum, start one canary instance
against that path, and validate immutable artifact bytes. Only after the canary
passes should the deployment configuration point at the restored file. Keep
the previous database file until the rollback window closes.

## 7. Back up and restore PostgreSQL

Use a dedicated SchemaHub database and PostgreSQL custom format. `pg_dump`
takes a transactionally consistent snapshot while the service stays online.
Use a client whose major version is the same as or newer than the server;
`pg_dump` intentionally refuses to dump a newer server with an older client:

```bash
export SCHEMAHUB_BACKUP=/var/backups/schemahub/schemahub-$(date -u +%Y%m%dT%H%M%SZ).dump
install -d -m 0700 "$(dirname "$SCHEMAHUB_BACKUP")"
pg_dump --format=custom --no-owner --no-acl \
  --dbname "$SCHEMAHUB_DATABASE_URL" --file "$SCHEMAHUB_BACKUP"
pg_restore --list "$SCHEMAHUB_BACKUP" >/dev/null
sha256sum "$SCHEMAHUB_BACKUP" > "$SCHEMAHUB_BACKUP.sha256"
```

Restore into a new empty database, never over the live database:

```bash
createdb --maintenance-db "$SCHEMAHUB_POSTGRES_ADMIN_URL" schemahub_restore
pg_restore --exit-on-error --no-owner --no-acl \
  --dbname "$SCHEMAHUB_RESTORE_URL" "$SCHEMAHUB_BACKUP"
```

Start a canary SchemaHub process with `storage.url` or `--db-url` set to
`$SCHEMAHUB_RESTORE_URL`. Startup verifies migration checksums and applies any
new forward migration. Confirm `/readyz`, resource counts, operation history,
and an immutable source/descriptor/generated artifact digest captured before
the backup.

## 8. Upgrade and rollback

Before upgrade:

1. Capture and verify a backup.
2. Record the current binary version, `_sqlx_migrations`, and one immutable
   artifact name plus SHA-256 digest.
3. Read the release notes and migration compatibility statement.
4. Exercise the change-create/validate/review/Apply and artifact-fetch workflow
   in staging.

Roll forward one canary, wait for readiness, compare the captured artifact, and
then roll the remaining instances. Mixed-version operation is allowed only
when the release notes explicitly declare it.

Application rollback is safe while all applied migrations remain readable by
the prior binary. Schema rollback is forward-only by default: restore the
pre-upgrade backup into a new database and cut over after verification. Do not
manually delete `_sqlx_migrations` rows or run destructive down SQL on the live
database.

## 9. Operate production JWT identity

Production deployments use `[auth.jwt]`; static `[auth].tokens` are only for
development and trusted-tailnet rehearsals. Follow `authentication.md` for the
complete required configuration and claims contract.

At startup, SchemaHub must load at least one usable configured signing key. It
then emits:

- `schemahub.auth.jwks_loaded` after the initial load;
- `schemahub.auth.jwks_refreshed` after an atomic replacement;
- `schemahub.auth.jwks_refresh_failed` while retaining a still-fresh last
  known-good set;
- `schemahub.auth.jwks_freshness_changed` when readiness crosses the stale-key
  boundary.

Alert on any sustained refresh failures and on
`schemahub_readiness_checks_total{result="auth_failure"}`. A transient refresh
failure is tolerated only until `max_stale_seconds`; after that, JWT requests
and readiness fail closed. Run the overlap/next-key/old-key-removal drill from
`authentication.md` before issuer changes and every release candidate. JWTs
remain valid until expiration while their signing key is trusted, so use short
access-token lifetimes and the issuer's revocation controls.

## 10. GC and recovery drill

GC is global at the object layer even when requested through one authorized
repository. Before sweeping, SchemaHub discovers every repository key present
in op/ref storage, marks every historical operation view and commit ancestry,
and preserves cross-repository objects. Normal mutations hold a shared
maintenance guard; GC holds the exclusive side for its full mark/sweep. The
PostgreSQL guard is a database-wide advisory lock, so separate server instances
participate in the same fence.

Normal publishers also acquire an exclusive repository publication guard for
the final operation-head load, merge, policy validation, and commit. PostgreSQL
implements this as a repository-keyed advisory lock across instances; unrelated
repositories remain concurrent. Memory/redb use a process-local mutex, matching
the embedded backend's single-process deployment model.

Quarterly, in a restored staging copy:

1. Create an orphan fixture and revisions in at least two repositories.
2. Run GC through one repository scope.
3. Restart SchemaHub.
4. Fetch both repositories' immutable artifacts and compare digests.
5. Run `undo` on a pre-GC operation and verify the older schema is readable.
6. Record duration, deleted-object count, and evidence in the release log.

The release suite automates this drill for redb and PostgreSQL, including
cross-repository retention, restart, and post-GC undo.

## 11. Acceptance checklist

- Liveness stays `200`; readiness changes to `503` during drain.
- Production JWTs resolve to the expected prefixed human/agent/service identity;
  key rotation succeeds, and stale keys produce readiness `503`.
- Standard gRPC overall health reports the matching lifecycle state.
- Structured events contain correlation IDs but no credentials/schema source.
- Metrics scrape parses and request/readiness counters advance.
- Migration versions and checksums match the binary.
- A verified backup exists outside the primary failure domain.
- Restored ChangeRecords, projects, repositories, roles, JJ history, and
  immutable artifact bytes match the source deployment.
- GC preserves both current and historical cross-repository data.
