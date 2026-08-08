<!-- agent-updated: 2026-07-30T04:16:42Z -->
# SchemaHub GUI

SchemaHub includes a React operator console in `apps/schemahub-gui`. It reads
persisted resources, records human/agent change intent, authors executable
whole-schema source replacements and deletions, advances the shared
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
|-- src/App.tsx               # AppShell, sidebar navigation, lazy route table
|-- src/api/
|   |-- types.ts              # Browser-facing DTOs
|   |-- client.ts             # SchemaHubClient interface
|   |-- httpClient.ts         # HTTP/BFF client implementation
|   |-- mockClient.ts         # Mock data and async client implementation
|   |-- index.ts              # Selects HTTP or mock client
|   `-- queries.ts            # TanStack Query hooks
|-- src/pages/                # Route-level screens
|-- src/components/           # Reusable UI surfaces, including typed edit authoring
|-- src/theme.ts              # Mantine theme
`-- src/styles.css            # App layout styles

apps/browser-cdp.mjs          # Remote CDP discovery and host normalization
```

### Stack

| Layer | Implementation |
|---|---|
| Build | Vite |
| UI | React 18 + TypeScript |
| Component library | Mantine |
| Data fetching | TanStack Query |
| Routing | React Router |
| Code viewer | Self-contained read-only source viewer |
| Icons | lucide-react |
| Package manager | pnpm |

The production build contains every code-viewer asset. Its bundle contract
rejects known remote CDN hosts as well as an oversized entry chunk, so an
archive or container does not silently depend on third-party JavaScript at
runtime.

### Data Boundary

Pages do not call transport APIs directly. They use hooks in `src/api/queries.ts`, which call the `SchemaHubClient` interface in `src/api/client.ts`.

Default implementation:

```text
Page -> useQuery/useInfiniteQuery/useMutation hook -> SchemaHubClient -> HttpSchemaHubClient -> HTTP BFF -> Core
```

Explicit demo implementation:

```text
Page -> useQuery/useInfiniteQuery/useMutation hook -> SchemaHubClient -> MockSchemaHubClient
```

The browser-facing DTOs intentionally differ from internal Rust storage types.
Keep UI DTOs workflow-oriented and adapt gRPC responses at the client or BFF
boundary. Project, repository, dashboard, and ChangeRecord list DTOs are
bounded pages, not arrays of an entire catalog or repository projection.
TanStack Query retains each requested page, sends the preceding
`nextPageToken` unchanged, and only requests another page when the operator
selects the continuation control. Repository deep links use a page of size one
with the repository name as a prefix, then require an exact-name match.

## Implemented Screens

| Route | Screen |
|---|---|
| `/projects` | Incrementally paged project list with visibility, caller role, and recent activity |
| `/projects/:project` | Incrementally paged persisted repositories and runtime policy |
| `/projects/:project/repos/:repo` | Repo dashboard with schemas, refs, activity, and compatibility summary |
| `/projects/:project/repos/:repo/changes` | Durable human/agent proposals with optional executable source/deletion edits |
| `/projects/:project/repos/:repo/changes/:changeId` | ETag-protected draft editing, validation, readiness, review, Apply, and receipt workflow |
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
pnpm run test:bundle
pnpm run test:cdp
```

Every operator page is a route-level lazy boundary, so the initial shell does
not eagerly download page-only workflows such as source editing or schema
inspection. CI rejects a production entry chunk above 450,000 bytes; set
`SCHEMAHUB_GUI_MAX_ENTRY_BYTES` only when deliberately rehearsing another
budget. The mock-browser acceptance first proves project, repository, and
dashboard schema rows arrive only after their continuation buttons are
selected. After browser authoring it returns through SPA navigation and proves
the newer proposal appears only after the indexed ChangeRecord continuation.
It also fixes the viewport at 930 pixels before authoring and asserts that the
identity control remains inside the 56-pixel header, followed by the
self-contained source viewer's keyboard, line-number, scrolling, and
no-third-party-resource contract.

The live acceptance resolves the identity button through its exact
`Identity: <display name>` accessible name while switching between the
delegated agent and independent human. Its `finally` path closes pages,
contexts, and both local or remote browser connections, so neutral-CDP runs
cannot remain attached after success.

Run the frozen dependency audit with:

```bash
pnpm audit --audit-level low
```

`pnpm-workspace.yaml` locks patched esbuild and PostCSS transitive versions.
It contains exactly one advisory exception, `GHSA-qwww-vcr4-c8h2`, which
applies only to React Router's unstable server-side RSC APIs. This GUI is a
client-only Vite bundle using `BrowserRouter`; it imports no RSC or
server-action surface. The repository dependency-policy test rejects another
ignored advisory, a server/RSC import, vulnerable override drift, or removal
of the GUI audit from CI.

## Same-release production serving

Every native release archive includes the locked production build under
`schemahub-gui/`. Serve it from the same HTTP listener as the BFF:

```bash
schemahub-server \
  --listen "$TAILSCALE_IP:50051" \
  --http-listen "$TAILSCALE_IP:8080" \
  --gui-dir ./schemahub-gui \
  --config schemahub.toml
```

The equivalent persistent configuration is:

```toml
[http]
gui_dir = "/absolute/path/to/schemahub-gui"
```

`gui_dir` fails startup unless it contains a regular `index.html`, an `assets`
directory, and only regular files/directories throughout the complete tree.
Symbolic links—including a linked `assets` root, nested asset, or favicon—are
rejected before the listener starts so the static service cannot read outside
the configured root. The directory must remain immutable while the server
runs. It is also rejected when the HTTP listener is disabled. The release
container places the exact read-only build in `/usr/share/schemahub/gui` and
enables it by default. With port 8080 mapped to the Tailscale interface, the
console is available at
`http://shuoze25-yuacx.tail8f3b66.ts.net:8080/`.

The server returns the SPA entry for `/`, `/projects`, every nested
`/projects/...` route, and `/admin`. Only successful assets whose filename ends
in Vite's `-<eight-character URL-safe content hash>.<extension>` shape receive
`Cache-Control: public, max-age=31536000, immutable`; successful unhashed
assets and HTML receive `no-cache`. Static routes never carry the GUI-BFF
classification header, and unknown `/api/*` routes remain API `404` responses
rather than falling back to HTML. Successful GUI responses also receive a
self-only content security policy, framing denial, browser feature restrictions,
MIME-sniffing protection, and a same-origin referrer policy. The policy permits
inline styles because Mantine renders dynamic style attributes, but it does not
permit inline scripts or third-party runtime origins.

Exercise source creation, validation invalidation, ETag-protected editing, and
schema deletion in a real browser against the opt-in mock client:

```bash
export SCHEMAHUB_GUI_URL="http://$TAILSCALE_HOST:5173"
VITE_SCHEMAHUB_USE_MOCKS=true pnpm run dev
# From another shell:
pnpm run test:browser
```

The browser smoke connects to the neutral Ubuntu GUI's Playwright-compatible
CDP listener at
`http://ubuntu-gui-browser-arm2.tail8f3b66.ts.net:9223` by default. Set
`PLAYWRIGHT_CDP_ENDPOINT` to override the remote CDP endpoint or
`PLAYWRIGHT_CHROMIUM_EXECUTABLE` to launch a local Chromium-compatible binary;
CI uses the hosted runner's Google Chrome. The shared resolver fetches
`/json/version` and rewrites Chrome's advertised loopback WebSocket onto the
configured HTTP(S) CDP host; direct `ws:` and `wss:` endpoints remain
supported. Interactive Pwright sessions use the same neutral listener from an
isolated working directory.

Exercise the governed workflow against the real release server, HTTP BFF,
redb, and Vite. The runner creates an isolated private repository, has a
delegated agent author Protobuf source in Chromium, proves Apply fails before
review, switches to an independent human reviewer, applies as the agent, and
then verifies audit and descriptor identity after a server restart:

```bash
export SCHEMAHUB_GUI_URL="http://$TAILSCALE_HOST:5173"
./scripts/run-live-browser-smoke.sh
```

The live runner writes its evidence beneath a temporary directory by default.
Set `SCHEMAHUB_CODELAB_EVIDENCE_DIR` to retain it at an explicit path. CI uses
an isolated loopback fallback, uploads only the sanitized `result.json` and
browser screenshot, and keeps credential-bearing runtime files out of the
artifact.

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
| `GET /api/projects?pageSize=&pageToken=&namePrefix=` | Bounded visible-project page and opaque continuation |
| `GET /api/projects/:project/repos?pageSize=&pageToken=&namePrefix=` | Bounded persisted-repository page, runtime policy, and opaque continuation |
| `GET /api/projects/:project/repos/:repo/dashboard?ref=&pageSize=&pageToken=` | Bounded schema/branch/tag dashboard page, exact conflict counts, immutable schema snapshot, and opaque continuation |
| `GET /api/projects/:project/repos/:repo/changes?pageSize=&pageToken=&status=` | Bounded, source-redacted ChangeRecord page over the repository/status index |
| `POST /api/projects/:project/repos/:repo/changes` | Create a note-only or executable ChangeRecord |
| `GET /api/projects/:project/repos/:repo/changes/:id` | Change detail shared with gRPC/CLI, including source payloads for browser-editable replacements |
| `PATCH /api/projects/:project/repos/:repo/changes/:id` | Replace a draft's source/deletion edit list under ETag concurrency control |
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
- Bounded project/repository page DTOs backed by Core catalog ranges. Tokens
  are bound to catalog kind, project scope, and name prefix; a token from one
  route or filter is rejected by another.
- A composite repository-dashboard page. Its token binds project, repository,
  and ref expression; advances bounded schema, branch, and tag cursors
  together; and carries the first page's resolved commit so later schema
  summaries cannot cross a moving bookmark. Conflict totals are exact without
  collecting the repository-wide conflict list. Selected schema objects and
  repository-local names batch-load in one immutable traversal; dependency
  totals count unique compiler-reported direct imports without target
  traversal.
- Bounded ChangeRecord list pages over Core's repository/status index. Tokens
  cannot cross repository or lifecycle filter, and list records omit complete
  replacement-source payloads.
- Explicit React continuations on dashboard/ref consumers and the proposal
  list. Loaded metrics are labeled as such rather than pretending one page is
  a repository-wide total.
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
- The GUI authors whole-schema source replacements and schema deletions directly
  and can convert an existing note into an executable draft. Compiler-specific
  granular operation builders remain available through the CLI and public
  `schemahub.v1` ChangeService.
- Search is repository-scoped; project-wide and cross-project indexes are D6+
  work.
- Project/repository selectors, repository dashboards, and ChangeRecord lists
  are bounded. They remain same-release GUI BFF projections outside the public
  1.x compatibility promise.
- CI runs both the mock executable-edit smoke and a live Chromium
  agent-author/human-review/agent-Apply/restart-serving acceptance. Isolated
  component tests have not been added yet.
