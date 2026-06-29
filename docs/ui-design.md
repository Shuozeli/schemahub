<!-- agent-updated: 2026-06-24T04:39:10Z -->

# SchemaHub UI Design

SchemaHub v1 is CLI + gRPC only. This document defines the first web console we should build on top of the existing gRPC API. The UI should feel like an operational registry console, not a landing page: dense, searchable, auditable, and optimized for repeated schema review.

The first implementation target is a React + Vite application under the SchemaHub repo. It should start as a read-mostly auditor/operator console with mock data and a typed API boundary, then swap the mock client for a BFF or gRPC-web adapter once the HTTP boundary is chosen.

Implementation status: the first mock console now lives in `apps/schemahub-gui`. See `docs/gui.md` for the implemented architecture, route map, run commands, Tailscale preview setup, and troubleshooting notes. This document remains the product/design source for the UI roadmap.

## Product Positioning

The UI is for engineers, platform owners, and auditors who need to inspect schema state, review changes, verify compatibility, and understand registry history.

Primary jobs:

- Find a project, repo, schema, declaration, branch, tag, commit, or operation quickly.
- Inspect the current canonical schema source and generated descriptors.
- Compare refs and understand compatibility risk.
- Review who changed what, when, and through which operation.
- Execute safe schema workflows: create branch, apply mutation, merge, tag, undo, resolve conflict.

Non-goals for the first UI:

- Marketing site.
- Visual schema diagram editor.
- Replacing local IDE workflows.
- Building a custom parser in the browser.
- Exposing every raw protobuf field before the main registry workflows are usable.

## Technology Choice

Use this stack for the first GUI:

| Layer | Choice | Reason |
|---|---|---|
| App build | Vite + React + TypeScript | Fast local iteration, simple deployment artifact, no server framework assumption. |
| UI library | Mantine | Complete app components, good density, strong forms/modals/tables, easier custom operational style than Ant Design. |
| Data fetching | TanStack Query | Clear cache/loading/error model, easy mock-to-real migration. |
| Routing | TanStack Router or React Router | Route params map cleanly to project/repo/schema/ref resources. Use React Router if we want fewer moving parts. |
| Tables | Mantine Table first; TanStack Table later if needed | Start simple. Move to TanStack Table only when sorting/filtering/virtualization grows. |
| Code viewer | Monaco Editor | Schema source, generated code, and diff previews need real code ergonomics. |
| Icons | lucide-react | Consistent, lightweight operational icon set. |
| Tests | Vitest + Testing Library | Component and API-client tests without requiring a browser server. |

Why not Ant Design first:

- AntD is strong for enterprise CRUD, but the default visual language is heavier and more generic.
- SchemaHub needs source/diff/audit density more than form-heavy admin CRUD.
- Mantine gives us enough components while keeping the UI easier to tune.

Why not shadcn/ui first:

- shadcn is good when the design system itself is the product.
- For SchemaHub, the immediate risk is workflow correctness, not custom component styling.
- We can still use Radix patterns later if Mantine blocks a specific interaction.

## Feature Priority

Build in three phases. Each phase must be useful without the later phases.

### Phase 1: Auditor Read Console

Required:

- Project/repo selector shell.
- Repo dashboard with schema inventory, branch/tag summary, recent operations.
- Schema detail with canonical source, declaration list, dependencies, and codegen preview.
- Compare page with base/head refs and declaration-level changes.
- History page with commit log and operation log.
- Admin/readiness page showing server config and auth mode.

Deferred:

- Mutating schemas.
- Conflict resolution writes.
- Member management writes.
- Branch/tag creation.
- Merge/undo actions.

### Phase 2: Safe Write Workflows

Required:

- Create branch.
- Create tag.
- Upload/create schema.
- Update schema from file.
- Merge branch after compare review.
- Undo with confirmation and operation context.

Guardrails:

- Always show target project/repo/ref in mutation dialogs.
- Require typed confirmation for destructive actions.
- Show compatibility/conflict status before merge.

### Phase 3: Conflict and RBAC Management

Required:

- Conflict side-by-side viewer.
- Resolve conflict from chosen side or uploaded source.
- Project member table.
- Add/remove/set-role workflows.
- Role-aware disabled states.

## Information Architecture

Use a persistent three-part shell:

- Left sidebar: project/repo navigation and saved filters.
- Top bar: global search, active ref selector, auth identity, server health.
- Main workspace: resource-specific table, detail, diff, or editor surfaces.

Top-level routes:

| Route | Purpose |
|---|---|
| `/projects` | Project list, visibility, role summary, last activity |
| `/projects/:project` | Project overview, repos, members, permissions |
| `/projects/:project/repos/:repo` | Repo dashboard: branches, tags, recent commits, schemas |
| `/projects/:project/repos/:repo/schemas/:schema` | Schema source, declarations, dependencies, codegen |
| `/projects/:project/repos/:repo/compare` | Diff and compatibility review between refs |
| `/projects/:project/repos/:repo/history` | Commit log and operation log |
| `/projects/:project/repos/:repo/conflicts` | Conflict list and resolution workflow |
| `/admin` | Server config, storage backend, limits, auth mode |

Route conventions:

- Use route params for stable resource identity.
- Use query params for selected refs and filters.
- Example: `/projects/acme/repos/commerce/schemas/order.proto?ref=main&tab=codegen`
- Example: `/projects/acme/repos/commerce/compare?base=tag:v1.0.0&head=main&schema=order.proto`

## Navigation Model

The sidebar should be compact and stable:

- Project picker at the top.
- Repo list under the active project.
- Secondary navigation for the active repo: Schemas, Compare, History, Conflicts, Tags, Settings.

The active ref is a first-class selector in the top bar:

- Branches: `main`, `feature/foo`
- Tags: `tag:v1.0.0`
- Commits: `@<commit>`

The ref selector should be visible on schema, compare, history, and codegen pages because nearly every read is versioned.

## Visual Style

Use a restrained operations-console style:

- Background: neutral light surface, not dark-only.
- Navigation: compact, high contrast, low decoration.
- Cards: only for repeated objects or modal panels; do not nest cards.
- Tables: primary layout for projects, schemas, commits, operations, tags.
- Badges: small status tokens for format, compatibility, role, visibility, conflict state.
- Diff colors: muted green/red with strong text contrast.
- Icons: use recognizable icons for branch, tag, commit, lock, warning, code, download, merge, undo.

Avoid:

- Hero sections.
- Large decorative gradients.
- Marketing copy.
- Oversized empty-state illustrations.
- One-color purple/blue dashboard styling.

Color roles:

- Neutral surfaces for page background and panels.
- Blue only for primary navigation/action affordance.
- Green/red only for diff and compatibility outcomes.
- Amber for warnings, conflicts, and protected-branch caveats.
- Purple should not be a dominant theme.

Density:

- Tables should default to compact row height.
- Page titles should be work-surface scale, not hero scale.
- Empty states should be one line plus a direct action where applicable.

## Core Screens

### Project List

Purpose: choose a project and understand access at a glance.

Columns:

- Project
- Visibility
- My role
- Repos
- Last operation
- Last activity

Actions:

- Create project.
- Filter public/private/member projects.
- Open project.

Empty state should offer a compact create-project form if the current identity can create projects.

### Project Overview

Purpose: manage project-level repos and members.

Sections:

- Repo table: name, default branch, protected branches, compatibility direction, schemas, latest commit.
- Member table: identity, role, last changed.
- Visibility and auth mode summary.

Actions:

- Create repo.
- Add member.
- Change role.
- Remove member.

Guardrails:

- Last-owner removal should be blocked before submit.
- Maintainer/Owner-only actions should be disabled with a tooltip when the current role is insufficient.

### Repo Dashboard

Purpose: operational summary of one schema repo.

Primary panels:

- Recent operations.
- Recent commits.
- Schema inventory by format.
- Protected branches and compatibility policy.
- Open conflicts.

Main table: schemas

Columns:

- Schema path
- Format
- Declarations
- Dependencies
- Last commit
- Conflict status

Actions:

- Create schema.
- Import schema from file.
- Create branch.
- Create tag.
- Open compare view.

First implementation mock widgets:

- Schema count by format.
- Latest commit.
- Latest operation.
- Open conflicts count.
- Protected branch chips.

The dashboard should not use large metric cards nested inside decorative cards. Use compact summary strips and tables.

### Schema Detail

Purpose: inspect a versioned schema and its declarations.

Layout:

- Left rail inside main workspace: declaration list grouped by kind.
- Center: canonical source viewer with line numbers.
- Right panel: declaration detail, dependencies, references, codegen controls.

Tabs:

- Source
- Declarations
- Dependencies
- Codegen
- History

Source tab:

- Read-only canonical source by default.
- Format badge: Protobuf, FlatBuffers, OpenAPI.
- Ref badge: current branch/tag/commit.
- Copy/download actions.

Declarations tab:

- Table with declaration name, kind, summary, type references.
- Clicking a row scrolls/highlights the source region when line data exists.

Dependencies tab:

- Import graph table: imported schema path, resolved commit, status.
- Follow-type action for selected fields/types.

Codegen tab:

- Language selector.
- Preview generated artifact.
- Download descriptors/source.
- Show generated artifact metadata: format, selected ref, schema path.
- FlatBuffers Rust option: `rust_pluggable_buffer`.
- For Protobuf/OpenAPI, hide or disable `rust_pluggable_buffer` with a short tooltip.

Codegen controls:

| Control | Type | Behavior |
|---|---|---|
| Language | segmented control or select | `Rust`, `TypeScript` initially; show disabled unsupported languages only if useful. |
| Descriptor | icon button | Downloads or previews `GetDescriptors`. |
| Pluggable buffer | switch | Visible only for FlatBuffers + Rust. Sends `PreviewCodegenRequest.rust_pluggable_buffer = true`. |
| Preview | button | Calls preview endpoint, renders Monaco read-only result. |

### Compare

Purpose: review schema changes before merge or release.

Inputs:

- Base ref.
- Head ref.
- Optional schema path filter.

Output:

- Summary strip: added, modified, removed declarations; compatibility status.
- Schema diff list.
- Declaration-level diff viewer.
- Compatibility findings with severity and rule.

Actions:

- Create tag from head.
- Merge head into target branch.
- Open affected schema.
- Copy release notes.

The compare page should be the main review surface before a branch merge.

Compare layout:

- Top filter bar: base ref, head ref, schema path filter.
- Left result list: schema diffs and declaration changes.
- Center diff viewer.
- Right inspector: compatibility findings, affected dependencies, suggested next action.

No mutation should happen from Compare until the selected base/head and target branch are visibly confirmed.

### History

Purpose: answer audit questions.

Two tabs:

- Commits: content history.
- Operations: registry operation log.

Commit table:

- Commit
- Change ID
- Parents
- Author
- Message
- Ref pointers

Operation table:

- Operation ID
- Author
- Action
- Target resource
- Before/after commit
- Timestamp when available

Actions:

- Open commit.
- Compare commit against parent.
- Undo operation, if permitted.

The UI must explain the difference between commits and operations through labels and column names, not tutorial text.

History filters:

- Ref.
- Schema path.
- Declaration name.
- Author.
- Operation kind.

Operation detail drawer:

- Operation ID.
- Author.
- Action kind.
- Target resource.
- Before/after commit if available.
- Raw metadata, collapsed by default.

### Conflicts

Purpose: make JJ first-class conflicts visible and resolvable.

List columns:

- Schema
- Declaration
- Branch
- Conflict sides
- Last touched

Resolution workflow:

- Show base/ours/theirs or all conflict sides.
- Let user choose one side or paste/upload resolved source.
- Validate resolution through server.
- Commit resolution with author/message.

Do not hide conflicts behind generic merge failures. Conflict objects are a feature.

### Admin

Purpose: inspect server and policy state.

Sections:

- Server config: transaction limits, supported formats, storage backend.
- Auth mode: noop or bearer/RBAC.
- Compatibility policies.
- Protected branches.

This page should be read-only for the first release unless admin write RPCs are added.

## Critical Workflows

### Review and Merge a Schema Change

1. Open repo dashboard.
2. Select source branch in ref selector.
3. Open Compare.
4. Set base `main`, head `feature/...`.
5. Review declaration-level changes and compatibility.
6. Merge into `main`.
7. Verify new commit and operation entries.

### Release a Schema Version

1. Open Compare.
2. Compare previous release tag against `main`.
3. Review generated release notes.
4. Create a new tag from `main`.
5. Verify tag appears in ref selector and tag table.

### Audit a Breaking Change

1. Open History.
2. Filter operations by schema path or declaration.
3. Open the operation.
4. Compare before/after commit.
5. Inspect compatibility findings.
6. Open author and role context.

### Generate Client Artifacts

1. Open Schema Detail.
2. Select ref.
3. Open Codegen tab.
4. Select language.
5. Preview generated artifact.
6. Download artifact or descriptor.

## API Mapping

The first UI can use the same gRPC surface as the CLI. For browser delivery, add one of these adapters:

- gRPC-web proxy in front of `schemahub-server`.
- Thin BFF service that exposes HTTP/JSON and calls the existing gRPC API.

Recommended first implementation: BFF service. It keeps auth, streaming limitations, browser CORS, and protobuf `Any` error unpacking out of the frontend.

Initial UI API needs:

| UI feature | Existing service |
|---|---|
| Project/repo list and members | `ProjectService` |
| Schema create/update/delete | `SchemaService` |
| Schema source and declarations | `ExplorationService` |
| Branches, tags, merge, diff, commits | `RefService` |
| Operation log, undo, conflict resolve | `HistoryService` |
| Descriptors and codegen preview | `CodegenService` |
| Server limits/config | `AdminService` |

Gaps likely requiring API work:

- Persisted repo registry is still partial; the UI needs reliable repo listing.
- Merge response should expose conflicted declarations directly.
- Commit timestamps are currently incomplete in some derived history paths.
- Codegen artifact metadata should include filename and content type.
- Search should support cross-repo or project-wide mode later.

Frontend API boundary:

Define a typed `SchemaHubClient` interface in the GUI and implement two clients:

- `MockSchemaHubClient` for Phase 1 development.
- `HttpSchemaHubClient` for the future BFF.

The React app must not import generated Rust/protobuf server internals directly. The browser-facing DTOs should be stable UI models:

```ts
type RefName = string;

type SchemaSummary = {
  path: string;
  format: 'protobuf' | 'flatbuffers' | 'openapi';
  declarations: number;
  dependencies: number;
  conflictCount: number;
  lastCommit?: string;
};

type CodegenPreviewRequest = {
  project: string;
  repo: string;
  schemaPath: string;
  ref: RefName;
  language: 'rust' | 'typescript';
  rustPluggableBuffer?: boolean;
};
```

Initial client methods:

- `listProjects()`
- `listRepos(project)`
- `getRepoDashboard(project, repo, ref)`
- `listSchemas(project, repo, ref)`
- `getSchemaSource(project, repo, schemaPath, ref)`
- `listDeclarations(project, repo, schemaPath, ref)`
- `listDependencies(project, repo, schemaPath, ref)`
- `previewCodegen(request)`
- `getDescriptors(project, repo, schemaPath, ref)`
- `diff(project, repo, base, head, schemaPath?)`
- `listCommits(project, repo, ref, limit)`
- `listOperations(project, repo, limit)`
- `getServerConfig()`

## Component Set

Build these reusable components first:

### Shell Components

- `AppShell`: sidebar, top bar, content outlet.
- `ProjectRepoSwitcher`: compact selector for current project/repo.
- `GlobalSearch`: searches schemas, declarations, refs, operations.
- `RefSelector`: branch/tag/commit switcher with search.
- `ResourceHeader`: project/repo/schema title, ref selector, key actions.
- `ServerStatusBadge`: health/auth/storage status.

### Registry Components

- `SchemaTable`: schema inventory.
- `FormatBadge`: Protobuf/FlatBuffers/OpenAPI display.
- `DeclarationList`: grouped declaration navigation.
- `DeclarationSummaryPanel`: fields, enum values, services, docs.
- `DependencyTable`: imports and resolved commits.
- `RefBadge`: branch/tag/commit display with icon.

### Review Components

- `SourceViewer`: read-only Monaco code viewer with line numbers.
- `DiffViewer`: declaration-aware diff.
- `CompatibilityPanel`: rule findings and severity.
- `CompareRefBar`: base/head/schema filter controls.
- `ChangeList`: schema and declaration diff list.

### Audit Components

- `OperationLogTable`: audit table.
- `CommitGraphList`: commit list with parents/change IDs.
- `OperationDetailDrawer`: operation metadata and before/after links.
- `CommitDetailDrawer`: commit metadata and parent comparison.

### Action Components

- `CodegenPreview`: language selector, options, preview, download.
- `DescriptorDownloadButton`: calls descriptor endpoint.
- `ConflictResolver`: side picker plus resolved-source submission.
- `RoleGate`: permission-aware action wrapper.
- `ConfirmMutationDialog`: explicit target-resource confirmation.

### Component Implementation Notes

- Use Mantine `AppShell`, `NavLink`, `Table`, `Tabs`, `Drawer`, `Modal`, `Badge`, `SegmentedControl`, `Select`, `Switch`, `Tooltip`, and `ActionIcon`.
- Use lucide icons inside action buttons and nav rows.
- Use Monaco for source, generated code, and diffs; do not use plain `<textarea>` for read-only code surfaces.
- Keep all tables compact and keyboard reachable.

## Page-Level Component Trees

### Repo Dashboard

```text
RepoDashboardPage
  ResourceHeader
  RepoSummaryStrip
  Tabs
    SchemasTab
      SchemaTable
    ActivityTab
      OperationLogTable
      CommitGraphList
    RefsTab
      BranchTable
      TagTable
```

### Schema Detail

```text
SchemaDetailPage
  ResourceHeader
  Tabs
    SourceTab
      DeclarationList
      SourceViewer
      DeclarationSummaryPanel
    DependenciesTab
      DependencyTable
    CodegenTab
      CodegenPreview
    HistoryTab
      CommitGraphList
      OperationLogTable
```

### Compare

```text
ComparePage
  CompareRefBar
  CompareSummaryStrip
  ChangeList
  DiffViewer
  CompatibilityPanel
```

### History

```text
HistoryPage
  ResourceHeader
  HistoryFilters
  Tabs
    Commits
      CommitGraphList
      CommitDetailDrawer
    Operations
      OperationLogTable
      OperationDetailDrawer
```

## Layout Details

Desktop:

- Sidebar width: 260px.
- Top bar height: 48px.
- Main content max width: none for tables and diffs.
- Schema detail uses resizable split panes: declarations 280px, source flexible, details 360px.

Mobile/tablet:

- Sidebar collapses into a project/repo switcher.
- Tables become dense list rows.
- Source and diff viewers should stay horizontally scrollable rather than wrapping code.

Accessibility:

- Keyboard reachable menus and tabs.
- Visible focus states.
- Non-color indicators for compatibility and conflict states.
- Copy/download buttons must have accessible labels.

Loading and error states:

- Every table needs loading skeleton rows.
- Every API failure should show a compact error banner with retry.
- Empty states must name the current project/repo/ref so users know the filter context.
- Codegen preview should keep the previous successful artifact visible while a new request loads, with a small stale/loading indicator.

State model:

- Selected project/repo lives in URL params.
- Selected ref lives in query params.
- Active tab lives in query params when deep linking matters.
- Do not store resource identity only in React component state.

Mock data:

- Include one Protobuf schema with imported type.
- Include one FlatBuffers schema with `rustPluggableBuffer` codegen option.
- Include one OpenAPI schema where codegen is unsupported.
- Include branch/tag examples and at least one operation log entry.
- Include one compatibility warning and one first-class conflict in mock data, even before write flows exist.

## First Milestone

Build a read-mostly console before mutation-heavy workflows:

1. App shell and project/repo navigation.
2. Repo dashboard.
3. Schema detail with source/declarations/dependencies/codegen.
4. Compare page.
5. History page.

Implementation sequence:

1. Scaffold `apps/schemahub-gui` with Vite React TypeScript.
2. Install Mantine, lucide-react, TanStack Query, router, Monaco.
3. Create app shell, theme, route skeletons.
4. Add typed UI models and `MockSchemaHubClient`.
5. Implement repo dashboard and schema detail from mock data.
6. Implement codegen preview panel with FlatBuffers `rustPluggableBuffer` switch.
7. Implement compare/history read views.
8. Add smoke tests for routing and codegen option state.

After that, add:

1. Create schema/update schema.
2. Branch/tag creation.
3. Merge workflow.
4. Conflict resolution.
5. RBAC member management.

## Success Criteria

The UI is successful when an auditor can answer these questions without using the CLI:

- What schemas exist in this repo?
- What branch/tag/commit am I looking at?
- What changed between release `X` and `main`?
- Was the change compatible?
- Who performed the write operation?
- Which commit and operation recorded it?
- Can generated code be produced for this schema at this ref?

## Open Decisions

- Router: React Router is simpler; TanStack Router gives better typed params. Choose before scaffold.
- BFF location: same Rust server, separate Rust crate, or Node/TypeScript service.
- Monaco diff strategy: use Monaco's diff editor or precomputed server diff text plus source viewer.
- Repo listing: current server repo registry is partial; Phase 1 mock can model repos, but real UI needs a reliable repo listing endpoint.
- Auth: first UI can use a token input/profile selector; full login flow is out of scope until auth provider is chosen.
