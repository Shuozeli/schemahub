# SchemaHub GUI

SchemaHub includes an experimental React console in `apps/schemahub-gui`. It is a read-mostly auditor/operator UI for inspecting projects, repos, schemas, refs, generated code previews, and audit history.

The GUI is mock-first by default and can be pointed at the Rust server's HTTP/JSON BFF by setting `VITE_SCHEMAHUB_API_BASE`. The app is structured around a typed client interface so mock data and live data share the same page code.

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

Current implementation:

```text
Page -> useQuery/useMutation hook -> SchemaHubClient -> MockSchemaHubClient
```

Live implementation:

```text
Page -> useQuery/useMutation hook -> SchemaHubClient -> HttpSchemaHubClient -> schemahub HTTP BFF -> Core
```

The browser-facing DTOs intentionally differ from internal Rust storage types. Keep UI DTOs stable and workflow-oriented; adapt gRPC responses at the client or BFF boundary.

## Implemented Screens

| Route | Screen |
|---|---|
| `/projects` | Project list with visibility, role, repo count, and recent activity |
| `/projects/:project/repos/:repo` | Repo dashboard with schemas, refs, activity, and compatibility summary |
| `/projects/:project/repos/:repo/schemas/*` | Schema detail with source, declarations, dependencies, and codegen preview |
| `/projects/:project/repos/:repo/compare` | Ref compare with declaration-level compatibility changes |
| `/projects/:project/repos/:repo/history` | Commit log and operation log |
| `/admin` | Read-only server config and supported format summary |

The shell currently pins the demo workspace to `acme/commerce` in `src/App.tsx`. Replace that with a real project/repo switcher when the API supplies reliable repo listing.

## Running Locally

Install dependencies:

```bash
cd apps/schemahub-gui
pnpm install
```

Run the development server:

```bash
pnpm run dev
```

Run the development server against a live SchemaHub HTTP BFF:

```bash
VITE_SCHEMAHUB_API_BASE=http://localhost:8080 pnpm run dev
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

```bash
export TAILSCALE_IP="$(tailscale ip -4)"
cargo run --release -p schemahub-server -- \
  --listen "$TAILSCALE_IP:50051" \
  --http-listen "$TAILSCALE_IP:8080"
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

The HTTP BFF enables permissive CORS for local development. It is intended as a development bridge for the GUI, not a hardened public API gateway.

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

## Connecting to the Real Server

The server exposes a read-only HTTP/JSON BFF when started with `--http-listen`. The GUI selects the live client when `VITE_SCHEMAHUB_API_BASE` is set; otherwise it uses `MockSchemaHubClient`.

Current BFF routes:

| Route | Purpose |
|---|---|
| `GET /api/projects` | Project list |
| `GET /api/projects/:project/repos/:repo/dashboard?ref=main` | Repo dashboard |
| `GET /api/projects/:project/repos/:repo/schemas/*schema_path?ref=main` | Schema detail |
| `GET /api/projects/:project/repos/:repo/diff?base=main&head=feature` | Ref diff |
| `GET /api/projects/:project/repos/:repo/history?ref=main&limit=25` | Commit and operation history |
| `POST /api/codegen/preview` | Codegen preview |
| `GET /api/admin/config` | Server config summary |

Recommended BFF responsibilities:

- Auth token forwarding.
- CORS.
- gRPC status/error normalization.
- Protobuf `Any` or rich error unpacking.
- Browser-safe JSON DTOs.

Do not import Rust server internals into the React app. The GUI should remain a browser client with a narrow API boundary.

## Current Limitations

- Live mode requires `schemahub-server --http-listen`; direct browser gRPC is not supported.
- Workspace navigation is hard-coded to `acme/commerce`.
- Mutating workflows are not implemented.
- Search input is visual only.
- Descriptor download button is visual only.
- No component tests have been added yet.
