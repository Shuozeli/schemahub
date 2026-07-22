<!-- agent-updated: 2026-07-21T23:31:27Z -->
# Deploy a SchemaHub Release on Tailscale

This codelab deploys the prepared release container with embedded redb and runs
the core change-to-immutable-artifact acceptance path. It is suitable for a
release rehearsal. The primary path uses an externally issued JWT and rotating
JWKS; a static token may be substituted only for local evaluation and does not
satisfy the production identity release gate.

## 1. Resolve the network identity

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
test -n "$TAILSCALE_IP"
test -n "$TAILSCALE_HOST"
```

All host port mappings bind only to `TAILSCALE_IP`. Clients use the stable,
fully qualified `TAILSCALE_HOST` name.

## 2. Choose an immutable image

For a published release, resolve and record the manifest digest:

```bash
export SCHEMAHUB_IMAGE=ghcr.io/shuozeli/schemahub:0.9.0-rc.1
docker pull "$SCHEMAHUB_IMAGE"
docker image inspect "$SCHEMAHUB_IMAGE" --format '{{index .RepoDigests 0}}'
```

For a local rehearsal, build from the shared projects context using the command
in `docs/release.md`, then set `SCHEMAHUB_IMAGE=schemahub:rc-rehearsal`. The
Docker build admits only SchemaHub source; both compilers resolve from their
immutable Cargo Git revisions.

## 3. Create a protected production configuration

Create `schemahub.toml` with file mode `0600`. Replace the issuer, audience,
JWKS URL, identity prefix, and subject with values from the staging identity
provider:

```toml
[http]
max_request_body_bytes = 8388608
# Same-origin is the default. Add an exact HTTPS GUI origin only when needed:
# allowed_origins = ["https://schemahub-gui.example.com"]

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

[projects.acceptance]
visibility = "private"
owners = ["corp-oidc:release-owner-subject"]

[repos."acceptance/registry"]
default_bookmark = "main"
compatibility = "backward"
protected_bookmarks = ["main"]

[repos."acceptance/registry".review]
required_approvals = 0
require_change_record = true
```

SchemaHub validates the access token but does not issue one. Obtain a raw bearer
access token for `release-owner-subject` from the configured provider. Do not
include the literal `Bearer ` prefix in `SCHEMAHUB_TOKEN`; the CLI adds it.
Follow `authentication.md` for claims, identity mapping, and the key-rotation
drill. For an air-gapped issuer, use `jwks_file` and mount that public file
read-only instead of setting `jwks_url`.

An empty `[http].allowed_origins` list emits no cross-origin permission. If a
browser console is served from another origin, list that canonical `http(s)`
origin exactly (including any non-default port). Wildcards and cookie
credentials are not supported. Startup rejects malformed/duplicate origins and
body limits outside 1 KiB through 64 MiB.

## 4. Start the non-root container

```bash
docker volume create schemahub-release-data
docker run --detach \
  --name schemahub-release \
  --restart unless-stopped \
  --publish "$TAILSCALE_IP:50051:50051" \
  --publish "$TAILSCALE_IP:8080:8080" \
  --volume schemahub-release-data:/var/lib/schemahub \
  --volume "$PWD/schemahub.toml:/etc/schemahub/schemahub.toml:ro" \
  "$SCHEMAHUB_IMAGE" \
  --listen 0.0.0.0:50051 \
  --http-listen 0.0.0.0:8080 \
  --db /var/lib/schemahub/schemahub.redb \
  --config /etc/schemahub/schemahub.toml
```

The wildcard addresses are container-internal. The host publishes them only on
the Tailscale interface.

Verify through MagicDNS:

```bash
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/healthz" | jq
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/readyz" | jq
curl --fail --silent --show-error "http://$TAILSCALE_HOST:8080/metrics" \
  | grep schemahub_build_info
curl --fail --silent --show-error \
  "http://$TAILSCALE_HOST:8080/api/openapi.json" \
  | jq -e '.openapi == "3.1.0"
    and (.paths | length == 22)
    and .info["x-schemahub-public-api"] == "schemahub.v1"
    and .paths["/api/projects"]["x-schemahub-api-surface"] == "gui-bff"
    and .paths["/healthz"]["x-schemahub-api-surface"] == "operations"'
curl --fail --silent --show-error --dump-header - --output /dev/null \
  "http://$TAILSCALE_HOST:8080/api/openapi.json" \
  | tr -d '\r' | grep -Fx 'x-schemahub-api-surface: gui-bff'
```

Confirm the health version, container label, and intended tag agree. The BFF
header and OpenAPI extensions are the machine-readable ADR 0002 boundary;
operational probe responses must not carry the `gui-bff` header.

## 5. Apply a recorded change

```bash
export SCHEMAHUB_SERVER="http://$TAILSCALE_HOST:50051"
test -n "${SCHEMAHUB_TOKEN:-}" # raw externally issued access token

schemahub repo init acceptance/registry
CHANGE="$(schemahub change note acceptance/registry \
    --title "Add the release acceptance schema" \
    --description "Proves recorded intent and immutable serving" \
    --reference release-acceptance-run \
    --id release-acceptance \
    --json)"
CHANGE_NAME="$(printf '%s' "$CHANGE" | jq -r .name)"
CHANGE_ETAG="$(printf '%s' "$CHANGE" | jq -r .etag)"

CHANGE="$(schemahub change add-source "$CHANGE_NAME" \
  --etag "$CHANGE_ETAG" \
  --schema-path user.proto \
  --file tests/integration/user.proto \
  --json)"
CHANGE_ETAG="$(printf '%s' "$CHANGE" | jq -r .etag)"

CHANGE="$(schemahub change validate "$CHANGE_NAME" \
  --etag "$CHANGE_ETAG" --json)"
CHANGE_ETAG="$(printf '%s' "$CHANGE" | jq -r .etag)"

CHANGE="$(schemahub change ready "$CHANGE_NAME" \
  --etag "$CHANGE_ETAG" --json)"
CHANGE_ETAG="$(printf '%s' "$CHANGE" | jq -r .etag)"

schemahub change apply "$CHANGE_NAME" \
  --etag "$CHANGE_ETAG" \
  --request-id release-acceptance-apply \
  --json | jq
```

The same resource can be inspected or reviewed through the GUI. The broader
CLI/gRPC walkthrough is in `docs/codelab-cli-grpc.md`.

Resolve and persist the immutable coordinates:

```bash
schemahub artifact resolve acceptance/registry --at main --json \
  > acceptance-revision.json
export REVISION="$(jq -r '.name' acceptance-revision.json)"
schemahub artifact fetch "$REVISION" \
  --schema-path user.proto \
  --kind descriptors \
  --output acceptance.desc
sha256sum acceptance.desc > acceptance.desc.sha256
```

## 6. Restart and verify exact bytes

```bash
docker stop --timeout 35 schemahub-release
docker start schemahub-release

until curl --fail --silent "http://$TAILSCALE_HOST:8080/readyz" >/dev/null; do
  sleep 1
done

schemahub artifact fetch "$REVISION" \
  --schema-path user.proto \
  --kind descriptors \
  --output acceptance-after-restart.desc
sha256sum --check acceptance.desc.sha256
cmp acceptance.desc acceptance-after-restart.desc
```

The first successful fetch is durably stored before it returns. For an upgrade
rehearsal, repeat after replacing the container with the new image while
retaining the volume. The new process must return the stored bytes even if its
compiler would render differently; a digest mismatch is a failed release gate,
not a warning.

## 7. Back up and roll forward

Drain and stop the container before copying redb from the volume. Follow the
offline backup procedure in `docs/codelab-operations.md`. Keep the prior image
digest and backup until the rollback window closes. For PostgreSQL, use the
online `pg_dump`/fresh-database restore procedure from that runbook instead.

Do not publish a release based only on this redb rehearsal. The PostgreSQL
matrix, real-provider rotation/staleness drill, SBOM/checksum review, clean
compiler-ref build, and durable cross-release artifact-byte verification must
also pass.
