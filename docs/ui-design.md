<!-- agent-updated: 2026-07-30T04:16:42Z -->

# SchemaHub UI Design

SchemaHub includes a CLI, gRPC API, and HTTP-backed web console. This document
defines the console as an operational registry surface: dense, searchable,
auditable, and optimized for repeated schema review.

The implementation is a React + Vite application with a typed browser boundary.
The live Rust BFF is the default data path; an explicit mock client remains for
isolated demos.

Implementation status (2026-07-29): D5 is implemented in
`apps/schemahub-gui`. Real resource navigation, server-derived human/agent
identity, ChangeRecord lifecycle actions, repository search, immutable artifact
download, and conflict resolution all use Core-authorized BFF paths. Operator
pages are lazy route boundaries with a CI entry-bundle budget, and the exact
production build ships in native archives and the release container for
fail-fast same-origin serving. See
`docs/gui.md` for the concrete route and deployment contract. This document
remains the product/design source for later UI expansion.
Project and repository selectors now use TanStack infinite queries over
bounded BFF pages; explicit continuation controls preserve server cursors
without returning or decorating an entire catalog.

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
| App build | Vite + React + TypeScript | Fast local iteration, lazy route chunks, and one version-matched static artifact served by SchemaHub or another static host. |
| UI library | Mantine | Complete app components, good density, strong forms/modals/tables, easier custom operational style than Ant Design. |
| Data fetching | TanStack Query | Clear cache/loading/error model, easy mock-to-real migration. |
| Routing | TanStack Router or React Router | Route params map cleanly to project/repo/schema/ref resources. Use React Router if we want fewer moving parts. |
| Tables | Mantine Table first; TanStack Table later if needed | Start simple. Move to TanStack Table only when sorting/filtering/virtualization grows. |
| Code viewer | Self-contained source viewer | Schema source, generated code, and diff previews need line numbers, selection, and horizontal scrolling without a runtime CDN dependency. |
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
| `/projects` | Incrementally paged project list, visibility, role summary, last activity |
| `/projects/:project` | Incrementally paged repository list and runtime policy |
| `/projects/:project/repos/:repo` | Repo dashboard: branches, tags, recent commits, schemas |
| `/projects/:project/repos/:repo/changes` | Human/agent intent and executable change proposals |
| `/projects/:project/repos/:repo/changes/:changeId` | Validation, review, Apply, and immutable receipt |
| `/projects/:project/repos/:repo/schemas/:schema` | Schema source, declarations, dependencies, codegen |
| `/projects/:project/repos/:repo/compare` | Diff and compatibility review between refs |
| `/projects/:project/repos/:repo/history` | Commit log and operation log |
| `/projects/:project/repos/:repo/conflicts` | Conflict list and resolution workflow |
| `/projects/:project/repos/:repo/search` | Repository schemas, declarations, revisions, and changes |
| `/admin` | Server config, storage backend, limits, auth mode |

### Change Proposal Authoring

The change list creates either a note-only draft or an executable proposal.
Executable browser edits are typed as complete source replacement or schema
deletion. Each edit visibly names its repository-relative path and format;
source paths and formats must agree before the server persists the draft.

The change detail page can replace the editable source/deletion list only while
the record is a draft. It sends the displayed ETag, and the server clears the
old validation snapshot after a successful edit. Opaque compiler mutations
created through gRPC or the CLI remain visible and reviewable in the GUI but
cannot be rewritten by a browser that cannot round-trip their operation bytes.

This keeps the browser on the same ChangeRecord lifecycle rather than adding a
parallel direct-write path: author, validate, mark ready, review, Apply, and
retain the immutable receipt.

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
| Preview | button | Calls preview endpoint, renders the read-only source viewer. |

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

The CLI and browser use the same Core policy paths through different transport
adapters. Browser delivery uses the HTTP/JSON BFF in `schemahub-server`.

The BFF owns bearer forwarding, CORS, browser DTOs, and gRPC-equivalent error
semantics. It calls Core directly, so it does not introduce a second policy or
lifecycle implementation. Project and repository DTOs preserve Core's bounded
catalog pages, with opaque tokens bound to kind, project, and prefix; the
project DTO deliberately omits a repository count that would require an N+1
scan. Its OpenAPI 3.1 document is generated from the same
annotated handlers used for runtime routing and is available at
`/api/openapi.json`; `http-api.md` defines the drift and release contract. Per
ADR 0002, these unversioned routes are a same-release GUI-only BFF outside the
public API compatibility promise. Responses and generated path metadata expose
that classification; reusable integrations target `schemahub.v1` instead.

The maintained browser acceptance crosses this boundary with real processes:
a delegated agent authors executable source, a separate human approves it,
the agent applies it, and the test then reads the applied schema plus immutable
descriptor from the redb-backed server after restart. The deliberate
pre-review Apply must fail with the BFF's `412` policy response.

Initial UI API needs:

| UI feature | Existing service |
|---|---|
| Project/repo list and members | `ProjectService` |
| Schema create/update/delete | `SchemaService` |
| Schema source and declarations | `ExplorationService` |
| Branches, tags, merge, diff, commits | `RefService` |
| Operation log, undo, conflict resolve | `HistoryService` |
| Descriptors and codegen preview | `CodegenService` |
| Durable intent, validation, review, Apply | `ChangeService` |
| Immutable source/descriptors/generated code | `ServingService` |
| Server limits/config | `AdminService` |

Remaining API/UI expansion:

- Merge response should expose conflicted declarations directly.
- Commit timestamps are currently incomplete in some derived history paths.
- Artifact responses expose content type and digests; a future manifest can add
  server-selected filenames.
- Search should add project-wide/cross-project indexes and pagination later.

Frontend API boundary:

Define a typed `SchemaHubClient` interface in the GUI and implement two clients:

- `HttpSchemaHubClient` for the default live BFF.
- `MockSchemaHubClient` for explicit demo mode.

The React app must not import generated Rust/protobuf server internals directly. The browser-facing DTOs should be stable UI models:

```ts
type RefName = string;

type ProjectPage = {
  projects: ProjectSummary[];
  nextPageToken: string;
};

type RepoPage = {
  repositories: RepoSummary[];
  nextPageToken: string;
};

type RepoDashboardPage = {
  repo: RepoSummary;
  schemas: SchemaSummary[];
  branches: string[];
  tags: string[];
  resolvedCommit: string;
  nextPageToken: string;
};

type ChangePage = {
  changes: ChangeRecord[];
  nextPageToken: string;
};

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

- `listProjects(pageToken, pageSize)`
- `listRepos(project, pageToken, pageSize, namePrefix)`
- `getRepo(project, repo)` using one bounded exact-prefix page
- `getRepoDashboard(project, repo, ref, pageToken, pageSize)`
- `listChanges(project, repo, pageToken, pageSize, status?)`
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

- `SourceViewer`: self-contained read-only code viewer with line numbers.
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
- Use the native read-only source viewer for source, generated code, and diffs;
  do not use a plain `<textarea>` or a runtime CDN dependency for read-only
  code surfaces.
- Keep all tables compact and keyboard reachable.
- Keep the top bar single-line at every supported viewport. The repository
  search flexes between fixed brand and identity groups; the storage badge is
  desktop-only, and mobile identity retains an accessible label while showing
  only its icon.

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

Delivered implementation sequence:

1. Scaffold `apps/schemahub-gui` with Vite React TypeScript.
2. Install Mantine, lucide-react, TanStack Query, and the router.
3. Create app shell, theme, route skeletons.
4. Add typed UI models and `MockSchemaHubClient`.
5. Implement repo dashboard and schema detail from mock data.
6. Implement codegen preview panel with FlatBuffers `rustPluggableBuffer` switch.
7. Implement compare/history read views.
8. Replace pinned workspace data with persisted project/repository navigation.
9. Add authenticated ChangeRecord, search, conflict, and immutable artifact workflows.
10. Verify the BFF lifecycle and conflict paths with release-mode integration tests.
11. Exercise agent authoring, human approval, Apply, and restart identity in a
    real browser against the durable server.
12. Split operator routes, enforce the initial-entry budget, and ship the exact
    production artifact in every native archive and release container.

Later UI expansion can add:

1. Create schema/update schema.
2. Branch/tag creation.
3. Merge workflow.
4. RBAC member management.
5. Project-wide search and richer artifact manifests.

## Success Criteria

The UI is successful when an auditor can answer these questions without using the CLI:

- What schemas exist in this repo?
- What branch/tag/commit am I looking at?
- What changed between release `X` and `main`?
- Was the change compatible?
- Who performed the write operation?
- Which commit and operation recorded it?
- Can generated code be produced for this schema at this ref?

## Remaining Decisions

- Diff strategy: render precomputed server diff text in the source viewer.
- Repository dashboards and ChangeRecord lists remain same-release BFF
  projections, but their page DTOs and explicit incremental controls are now
  defined and bounded. Dashboard schema rows are summarized through one batch
  immutable-tree read rather than per-row repository scans. A future public
  REST surface still needs a separately versioned contract.
- Auth: replace the development token input with the selected production login provider.
- Testing: add isolated component-level accessibility coverage; governed mock
  and live browser workflows already run in CI.
