# SchemaHub GUI

SchemaHub includes an experimental React console in `apps/schemahub-gui`. It is a read-mostly auditor/operator UI for inspecting projects, repos, schemas, refs, generated code previews, and audit history.

The GUI is currently mock-first. It does not call the Rust gRPC server yet. The app is structured around a typed client interface so the mock client can later be replaced by an HTTP/BFF or gRPC-web adapter without rewriting the pages.

## Architecture

```
apps/schemahub-gui
|-- src/main.tsx              # React root, Mantine, QueryClient, router providers
|-- src/App.tsx               # AppShell, sidebar navigation, route table
|-- src/api/
|   |-- types.ts              # Browser-facing DTOs
|   |-- client.ts             # SchemaHubClient interface
|   |-- mockClient.ts         # Mock data and async client implementation
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

Future implementation:

```text
Page -> useQuery/useMutation hook -> SchemaHubClient -> HttpSchemaHubClient/BFF -> schemahub gRPC server
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

Build the production bundle:

```bash
pnpm run build
```

Preview a production build:

```bash
pnpm run preview
```

## Running on Tailscale

For this workspace, bind Vite to the Tailscale interface and allow the MagicDNS hostname:

```bash
cd apps/schemahub-gui
export TAILSCALE_IP="$(tailscale ip -4)"
export TAILSCALE_HOST="$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')"
pnpm run dev -- --force
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

The intended path is:

1. Add an `HttpSchemaHubClient` that implements `SchemaHubClient`.
2. Put browser transport details behind that client. Prefer a small BFF that calls the existing gRPC API.
3. Keep TanStack Query keys and route params unchanged.
4. Convert gRPC responses into the DTOs in `src/api/types.ts`.
5. Add loading/error states for real network failures before enabling write workflows.

Recommended BFF responsibilities:

- Auth token forwarding.
- CORS.
- gRPC status/error normalization.
- Protobuf `Any` or rich error unpacking.
- Browser-safe JSON DTOs.

Do not import Rust server internals into the React app. The GUI should remain a browser client with a narrow API boundary.

## Current Limitations

- Uses `MockSchemaHubClient`, not the live gRPC API.
- Workspace navigation is hard-coded to `acme/commerce`.
- Mutating workflows are not implemented.
- Search input is visual only.
- Descriptor download button is visual only.
- No component tests have been added yet.
