<!-- agent-updated: 2026-07-21T16:49:05Z -->
# SchemaHub GUI

SchemaHub includes a React operator console in `apps/schemahub-gui`. It reads
persisted resources, records human/agent change intent, advances the shared
ChangeRecord lifecycle, resolves declaration conflicts, downloads immutable
artifacts, and audits schema history.

The GUI selects the live Rust HTTP/JSON BFF by default. An empty
`VITE_SCHEMAHUB_API_BASE` means same-origin deployment; set it when Vite and the
BFF use different tailnet ports. Mock data is available only when explicitly
enabled with `VITE_SCHEMAHUB_USE_MOCKS=true`. Both implementations share the
same typed client interface and page code.

The unversioned `/api/*` surface is intentionally a GUI-only BFF, not the
public REST API. It is supported with the bundled GUI from the same release and
identifies itself with `x-schemahub-api-surface: gui-bff`. Durable external
integrations use `schemahub.v1` gRPC/protobuf or the CLI. See ADR 0002.

## Architecture

```
apps/schemahub-gui
|-- src/main.tsx              # React root, Mantine, QueryClient, router providers
|-- src/App.tsx               # AppShell, sidebar navigation, route table
|-- src/api/
|   |-- types.ts              # Browser-facing DTOs
|   |-- client.ts             # SchemaHubClient interface
|   |-- httpClient.ts         # HTTP/BFF client implementation
|   |-- mockClient.ts         # Mock data and async client implementation
|   |-- index.ts              # Selects HTTP or mock client
|   `-- queries.ts            # TanStack Query hooks
|-- src/pages/                # Route-level screens
|-- src/components/           # Reusable UI surfaces
|-- src/theme.ts              # Mantine theme
`-- src/styles.css            # App layout styles
```

### Stack

| Layer | Implementation |
|---|---|
| Build | Vite |
| UI | React 18 + TypeScript |
| Component library | Mantine |
| Data fetching | TanStack Query |
| Routing | React Router |
| Code viewer | Monaco Editor |
| Icons | lucide-react |
| Package manager | pnpm |

### Data Boundary

Pages do not call transport APIs directly. They use hooks in `src/api/queries.ts`, which call the `SchemaHubClient` interface in `src/api/client.ts`.

Default implementation:

```text
Page -> useQuery/useMutation hook -> SchemaHubClient -> HttpSchemaHubClient -> HTTP BFF -> Core
```

Explicit demo implementation:

```text
Page -> useQuery/useMutation hook -> SchemaHubClient -> MockSchemaHubClient
```

The browser-facing DTOs intentionally differ from internal Rust storage types. Keep UI DTOs stable and workflow-oriented; adapt gRPC responses at the client or BFF boundary.

## Implemented Screens

| Route | Screen |
|---|---|
| `/projects` | Project list with visibility, role, repo count, and recent activity |
| `/projects/:project` | Persisted repositories and runtime policy |
| `/projects/:project/repos/:repo` | Repo dashboard with schemas, refs, activity, and compatibility summary |
| `/projects/:project/repos/:repo/changes` | Durable human/agent notes with optional external references |
| `/projects/:project/repos/:repo/changes/:changeId` | Intent/references, validation, readiness, review, Apply, ETag, and receipt workflow |
| `/projects/:project/repos/:repo/conflicts` | Server-rendered conflict list and compiler-validated resolution |
| `/projects/:project/repos/:repo/search` | Schemas, declarations, revisions, and ChangeRecords at a ref |
| `/projects/:project/repos/:repo/schemas/*` | Schema detail with source, declarations, dependencies, and codegen preview |
| `/projects/:project/repos/:repo/compare` | Ref compare with declaration-level compatibility changes |
| `/projects/:project/repos/:repo/history` | Commit log and operation log |
| `/admin` | Read-only server config and supported format summary |

Routes and sidebar context derive from the selected persisted project and
repository. No production route pins an example workspace.

## Running on the tailnet

Install dependencies:

```bash
cd apps/schemahub-gui
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
pnpm install
```

Run the development server:

```bash
pnpm run dev
```

Run the development server against a live SchemaHub HTTP BFF:

```bash
VITE_SCHEMAHUB_API_BASE="http://${TAILSCALE_HOST}:8080" pnpm run dev
```

Run the opt-in demo client without a server:

```bash
VITE_SCHEMAHUB_USE_MOCKS=true pnpm run dev
```

Build the production bundle:

```bash
pnpm run build
```

Preview a production build:

```bash
pnpm run preview
```

## Running on Tailscale

Start SchemaHub with both gRPC and the HTTP BFF. The HTTP BFF is opt-in:

```toml
# schemahub.toml — replace the hostname with this machine's full MagicDNS name
[http]
allowed_origins = ["http://shuoze25-yuacx.tail8f3b66.ts.net:5173"]
max_request_body_bytes = 8388608
```

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
cargo run --release -p schemahub-server -- \
  --listen "$TAILSCALE_IP:50051" \
  --http-listen "$TAILSCALE_IP:8080" \
  --config schemahub.toml
```

Then bind Vite to the Tailscale interface, allow the MagicDNS hostname, and point it at the BFF:

```bash
cd apps/schemahub-gui
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
VITE_SCHEMAHUB_API_BASE="http://$TAILSCALE_HOST:8080" pnpm run dev -- --force
```

Open:

```text
http://$TAILSCALE_HOST:5173/
```

For example, on the current machine:

```text
http://shuoze25-yuacx.tail8f3b66.ts.net:5173/
```

`vite.config.ts` reads `TAILSCALE_IP` for `server.host` and `TAILSCALE_HOST` for `server.allowedHosts`.

The HTTP BFF emits no CORS permission headers by default. A separately hosted
GUI requires its exact canonical origin in `[http].allowed_origins`, including
the port. SchemaHub accepts only `http`/`https` origins without credentials,
paths, queries, or fragments; it never enables cookie credentials. Same-origin
deployments should leave the list empty. All JSON extractors share the bounded
`max_request_body_bytes` limit (8 MiB by default).

## Troubleshooting

### Blank Page After Dependency Changes

If the page loads HTML but stays blank, check a browser network request under `/node_modules/.vite/deps/`. Vite can return:

```text
504 Outdated Optimize Dep
```

Fix:

```bash
rm -rf node_modules/.vite
pnpm run dev -- --force
```

### MagicDNS Returns 403

Vite blocks unknown host headers. Make sure `TAILSCALE_HOST` is set before starting the server:

```bash
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
pnpm run dev -- --force
```

### Build Artifacts

`dist/`, `node_modules/`, Vite generated config files, and TypeScript build info are ignored by `apps/schemahub-gui/.gitignore`.

## HTTP BFF Contract

The server exposes the browser BFF when started with `--http-listen`. Bearer
tokens come from `VITE_SCHEMAHUB_TOKEN` or the browser identity menu and are
sent on every protected request. Actor and delegation fields are derived by the
server, never accepted from a form.

Current BFF routes:

| Route | Purpose |
|---|---|
| `GET /api/openapi.json` | Generated OpenAPI 3.1 contract for this HTTP boundary |
| `GET /api/projects` | Project list |
| `GET /api/projects/:project/repos` | Persisted repository list and runtime policy |
| `GET /api/projects/:project/repos/:repo/dashboard?ref=branch` | Repo dashboard and real conflict counts |
| `GET/POST /api/projects/:project/repos/:repo/changes` | List or record note-only ChangeRecords |
| `GET /api/projects/:project/repos/:repo/changes/:id` | Change detail shared with gRPC/CLI |
| `POST /api/projects/:project/repos/:repo/changes/:id/actions/:action` | Validate, ready, approve, reject, apply, or abandon with ETag |
| `GET /api/projects/:project/repos/:repo/search?q=...&ref=branch` | Repository resource search |
| `GET /api/projects/:project/repos/:repo/conflicts` | List unresolved declarations |
| `GET /api/projects/:project/repos/:repo/conflicts/render` | Render competing declaration sides |
| `POST /api/projects/:project/repos/:repo/conflicts/resolve` | Parse, validate, and commit a resolution |
| `GET /api/projects/:project/repos/:repo/schemas/*schema_path?ref=branch` | Schema detail |
| `GET /api/projects/:project/repos/:repo/diff?base=branch&head=feature` | Ref diff |
| `GET /api/projects/:project/repos/:repo/history?ref=branch&limit=25` | Commit and operation history |
| `GET /api/projects/:project/repos/:repo/revisions/resolve?ref=branch` | Resolve to an immutable revision |
| `GET /api/projects/:project/repos/:repo/revisions/:commit/artifacts/*schema_path` | Cache-safe source, descriptor, or generated artifact |
| `POST /api/codegen/preview` | Codegen preview |
| `GET /api/session` | Server-derived human/agent/service identity |
| `GET /api/admin/config` | Server config summary |

Implemented BFF responsibilities:

- Auth token forwarding.
- Same-origin default plus an explicit trusted-origin allowlist.
- Bounded HTTP request bodies.
- gRPC status/error normalization.
- gRPC-equivalent HTTP status normalization.
- Browser-safe JSON DTOs.
- Handler-derived OpenAPI metadata; see `http-api.md` for generation and
  release packaging.
- Runtime and per-path OpenAPI classification as GUI-only, with the response
  header exposed to explicitly allowed browser origins.

Do not import Rust server internals into the React app. The GUI should remain a
browser client with a narrow API boundary, and deploy it with the matching
server release.

## Current Limitations

- Live mode requires `schemahub-server --http-listen`; direct browser gRPC is not supported.
- Browser token entry uses local storage and is a development credential flow,
  not a full OIDC/login integration.
- The GUI creates intent-only drafts. Executable mutation/source edits are
  attached with the CLI or ChangeService, then validated/reviewed/applied in
  either surface.
- Search is repository-scoped; project-wide and cross-project indexes are D6+
  work.
- No component tests have been added yet.
