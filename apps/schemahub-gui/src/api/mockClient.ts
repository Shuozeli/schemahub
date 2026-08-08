import type {
  ArtifactDownload,
  ArtifactDownloadRequest,
  ChangeAction,
  ChangeActionRequest,
  ChangePage,
  ChangeRecord,
  ConflictDetail,
  ConflictList,
  CodegenPreview,
  CodegenPreviewRequest,
  CommitEntry,
  CreateChangeRequest,
  DiffResult,
  OperationEntry,
  ProjectPage,
  ProjectSummary,
  RepoDashboardPage,
  RepoPage,
  RepoSummary,
  ResolveConflictRequest,
  ResolveConflictResult,
  SchemaDetail,
  SearchResponse,
  ServerConfig,
  SessionInfo,
  UpdateChangeEditsRequest,
} from './types';
import type { SchemaHubClient } from './client';

const wait = (ms = 120) => new Promise((resolve) => window.setTimeout(resolve, ms));

const commits: CommitEntry[] = [
  {
    commit: '6ddcd8c5b0dd',
    changeId: 'zzxqvlnr',
    parents: ['8e67246bb26d', '4398b6668712'],
    author: 'schemahub-cli',
    message: 'merge shipping note',
    timestamp: '2026-06-24T04:14:21Z',
  },
  {
    commit: '8e67246bb26d',
    changeId: 'mrvkzotp',
    parents: ['4398b6668712'],
    author: 'schemahub-cli',
    message: 'add field Order.shipping_note',
    timestamp: '2026-06-24T04:13:02Z',
  },
  {
    commit: '4398b6668712',
    changeId: 'lvxqzppq',
    parents: ['0ed04ccce6c3'],
    author: 'schemahub-cli',
    message: 'create schema build_record.fbs',
    timestamp: '2026-06-24T04:10:16Z',
  },
];

const operations: OperationEntry[] = [
  {
    opId: 'op_01jyt9merge',
    author: 'schemahub-cli',
    action: 'MergeBranch',
    target: 'acme/commerce feature/shipping-note -> main',
    before: '4398b6668712',
    after: '6ddcd8c5b0dd',
    timestamp: '2026-06-24T04:14:21Z',
  },
  {
    opId: 'op_01jyt9field',
    author: 'schemahub-cli',
    action: 'ApplyMutation',
    target: 'acme/commerce/order.proto Order.shipping_note',
    before: '4398b6668712',
    after: '8e67246bb26d',
    timestamp: '2026-06-24T04:13:02Z',
  },
  {
    opId: 'op_01jyt9tag',
    author: 'schemahub-cli',
    action: 'CreateTag',
    target: 'acme/commerce tag:release-2026-06-05',
    after: '4398b6668712',
    timestamp: '2026-06-24T04:11:37Z',
  },
];

const orderSource = `syntax = "proto3";
package commerce.v1;

import "acme/commerce/common.proto";

message Order {
  string id = 1;
  Money total = 2;
  string shipping_note = 3;
}
`;

const flatBuffersSource = `namespace acme.commerce;

table BuildRecord {
  id: string;
  count: int;
}

root_type BuildRecord;
`;

const openApiSource = `openapi: 3.1.0
info:
  title: Commerce API
  version: 2026-06-24
paths:
  /orders/{id}:
    get:
      responses:
        "200":
          description: Order response
`;

const schemaDetails: Record<string, SchemaDetail> = {
  'order.proto': {
    path: 'order.proto',
    format: 'protobuf',
    source: orderSource,
    declarations: [
      {
        name: 'Order',
        kind: 'message',
        detail: '3 fields, imports Money from common.proto',
        refs: ['Money'],
      },
    ],
    dependencies: [
      {
        importingSchema: 'order.proto',
        importPath: 'acme/commerce/common.proto',
        resolvedCommit: 'fa50bf9d13f2',
        status: 'resolved',
      },
    ],
  },
  'build_record.fbs': {
    path: 'build_record.fbs',
    format: 'flatbuffers',
    source: flatBuffersSource,
    declarations: [
      {
        name: 'BuildRecord',
        kind: 'table',
        detail: 'root table with id:string and count:int',
        refs: [],
      },
    ],
    dependencies: [],
  },
  'commerce.yaml': {
    path: 'commerce.yaml',
    format: 'openapi',
    source: openApiSource,
    declarations: [
      {
        name: 'path:/orders/{id}',
        kind: 'path',
        detail: 'GET order by id',
        refs: ['schema:Order'],
      },
    ],
    dependencies: [],
  },
};

export class MockSchemaHubClient implements SchemaHubClient {
  private readonly changes: ChangeRecord[] = [
    {
      name: 'projects/acme/repos/commerce/changes/shipping-note',
      project: 'acme',
      repo: 'commerce',
      targetBookmark: 'main',
      title: 'Add shipping note to stored orders',
      description: 'Agent-observed producer field ready for human review.',
      externalReferences: ['INC-2048', 'https://tracker.example.test/issues/2048'],
      edits: [{ kind: 'mutation', schemaPath: 'order.proto', formatId: 'protobuf' }],
      createdBy: {
        identity: 'demo-agent',
        displayName: 'Demo Schema Agent',
        kind: 'agent',
        delegatedBy: 'demo-owner',
      },
      status: 'ready',
      validation: {
        valid: true,
        resolvedBaseCommit: '4398b6668712',
        editDigest: 'sha256:demo-shipping-note',
        issues: [],
        validatedAtUnixMs: Date.parse('2026-06-24T04:12:00Z'),
        validatorVersion: 'schemahub-validator-v1',
      },
      reviews: [],
      etag: 'v2',
      createTimeUnixMs: Date.parse('2026-06-24T04:11:00Z'),
      updateTimeUnixMs: Date.parse('2026-06-24T04:12:00Z'),
    },
  ];

  async listProjects(pageToken = '', pageSize = 50): Promise<ProjectPage> {
    await wait();
    const projects: ProjectSummary[] = [
      {
        name: 'acme',
        visibility: 'public',
        role: 'Owner',
        lastOperation: 'MergeBranch',
        lastActivity: '2026-06-24T04:14:21Z',
      },
      {
        name: 'platform',
        visibility: 'private',
        role: 'Maintainer',
        lastOperation: 'CreateTag',
        lastActivity: '2026-06-23T18:41:00Z',
      },
    ];
    const offset = Number.parseInt(pageToken || '0', 10) || 0;
    const effectivePageSize = Math.min(1, Math.max(1, pageSize));
    const end = Math.min(projects.length, offset + effectivePageSize);
    return {
      projects: projects.slice(offset, end),
      nextPageToken: end < projects.length ? end.toString() : '',
    };
  }

  async getSession(): Promise<SessionInfo> {
    await wait();
    return {
      authenticated: true,
      id: 'demo-agent',
      display: 'Demo Schema Agent',
      kind: 'agent',
      delegatedBy: 'demo-owner',
    };
  }

  async listChanges(
    project: string,
    repo: string,
    pageToken = '',
    pageSize = 50,
    status = '',
  ): Promise<ChangePage> {
    await wait();
    const changes = this.changes
      .filter(
        (change) =>
          change.project === project &&
          change.repo === repo &&
          (!status || change.status === status),
      )
      .sort(
        (left, right) =>
          left.createTimeUnixMs - right.createTimeUnixMs || left.name.localeCompare(right.name),
      );
    const offset = Number.parseInt(pageToken || '0', 10) || 0;
    const effectivePageSize = Math.min(1, Math.max(1, pageSize));
    const end = Math.min(changes.length, offset + effectivePageSize);
    return {
      changes: changes.slice(offset, end),
      nextPageToken: end < changes.length ? end.toString() : '',
    };
  }

  async getChange(project: string, repo: string, changeId: string): Promise<ChangeRecord> {
    await wait();
    const name = `projects/${project}/repos/${repo}/changes/${changeId}`;
    const change = this.changes.find((candidate) => candidate.name === name);
    if (!change) throw new Error(`change record not found: ${name}`);
    return change;
  }

  async createChange(
    project: string,
    repo: string,
    request: CreateChangeRequest,
  ): Promise<ChangeRecord> {
    await wait();
    const id = request.changeId || `note-${Date.now()}`;
    const latestStoredTime = this.changes.reduce(
      (latest, change) => Math.max(latest, change.updateTimeUnixMs),
      0,
    );
    const now = Math.max(Date.now(), latestStoredTime + 1);
    const change: ChangeRecord = {
      name: `projects/${project}/repos/${repo}/changes/${id}`,
      project,
      repo,
      targetBookmark: request.targetBookmark,
      baseRevision: request.baseRevision,
      title: request.title,
      description: request.description,
      externalReferences: request.externalReferences,
      edits: request.edits,
      createdBy: {
        identity: 'demo-agent',
        displayName: 'Demo Schema Agent',
        kind: 'agent',
        delegatedBy: 'demo-owner',
      },
      status: 'draft',
      reviews: [],
      etag: 'v1',
      createTimeUnixMs: now,
      updateTimeUnixMs: now,
    };
    this.changes.push(change);
    return change;
  }

  async updateChangeEdits(
    project: string,
    repo: string,
    changeId: string,
    request: UpdateChangeEditsRequest,
  ): Promise<ChangeRecord> {
    await wait();
    const current = await this.getChange(project, repo, changeId);
    if (current.status !== 'draft') throw new Error('only draft changes can be edited');
    if (current.etag !== request.etag) throw new Error('change record etag mismatch');
    const next: ChangeRecord = {
      ...current,
      edits: request.edits,
      validation: undefined,
      etag: `v${Number(current.etag.slice(1)) + 1}`,
      updateTimeUnixMs: Date.now(),
    };
    const index = this.changes.indexOf(current);
    this.changes[index] = next;
    return next;
  }

  async changeAction(
    project: string,
    repo: string,
    changeId: string,
    action: ChangeAction,
    request: ChangeActionRequest,
  ): Promise<ChangeRecord> {
    await wait();
    const current = await this.getChange(project, repo, changeId);
    if (current.etag !== request.etag) throw new Error('change record etag mismatch');
    const next: ChangeRecord = {
      ...current,
      etag: `v${Number(current.etag.slice(1)) + 1}`,
      updateTimeUnixMs: Date.now(),
    };
    if (action === 'validate') {
      next.validation = {
        valid: current.edits.length > 0,
        resolvedBaseCommit: '4398b6668712',
        editDigest: 'sha256:demo',
        issues:
          current.edits.length > 0
            ? []
            : [{ code: 'empty_change', message: 'A note-only draft has no executable edits.' }],
        validatedAtUnixMs: Date.now(),
        validatorVersion: 'schemahub-validator-v1',
      };
    } else if (action === 'ready') {
      if (!current.validation?.valid) throw new Error('change requires passing validation');
      next.status = 'ready';
    } else if (action === 'approve' || action === 'reject') {
      next.reviews = [
        ...current.reviews,
        {
          reviewer: { identity: 'demo-owner', displayName: 'Demo Owner', kind: 'human' },
          decision: action === 'approve' ? 'approved' : 'rejected',
          reason: request.reason || '',
          createTimeUnixMs: Date.now(),
        },
      ];
      if (action === 'reject') next.status = 'rejected';
    } else if (action === 'apply') {
      next.status = 'applied';
      next.applyResult = {
        commitId: 'demo-applied-commit',
        changeId: 'demo-jj-change',
        operationId: 'demo-operation',
        conflictedDeclarations: [],
      };
    } else if (action === 'abandon') {
      next.status = 'abandoned';
    }
    const index = this.changes.indexOf(current);
    this.changes[index] = next;
    return next;
  }

  async search(
    project: string,
    repo: string,
    query: string,
    ref: string,
    limit = 50,
  ): Promise<SearchResponse> {
    await wait();
    const needle = query.toLowerCase();
    const results: SearchResponse['results'] = [];
    for (const [schemaPath, schema] of Object.entries(schemaDetails)) {
      if (schemaPath.toLowerCase().includes(needle)) {
        results.push({
          kind: 'schema',
          title: schemaPath,
          description: `${schema.format} schema`,
          schemaPath,
        });
      }
      for (const declaration of schema.declarations) {
        if (declaration.name.toLowerCase().includes(needle)) {
          results.push({
            kind: 'declaration',
            title: declaration.name,
            description: declaration.detail || declaration.kind,
            schemaPath,
            declarationName: declaration.name,
          });
        }
      }
    }
    for (const commit of commits) {
      if (`${commit.commit} ${commit.changeId} ${commit.message}`.toLowerCase().includes(needle)) {
        results.push({
          kind: 'revision',
          title: commit.message,
          description: `${commit.author} · ${commit.timestamp}`,
          revision: commit.commit,
        });
      }
    }
    for (const change of this.changes.filter(
      (candidate) => candidate.project === project && candidate.repo === repo,
    )) {
      if (
        `${change.name} ${change.title} ${change.description} ${change.externalReferences.join(' ')}`
          .toLowerCase()
          .includes(needle)
      ) {
        const parts = change.name.split('/');
        results.push({
          kind: 'change',
          title: change.title,
          description: change.description,
          changeId: parts[parts.length - 1],
          status: change.status,
        });
      }
    }
    return { query, ref, results: results.slice(0, limit) };
  }

  async downloadArtifact(request: ArtifactDownloadRequest): Promise<ArtifactDownload> {
    await wait();
    const source = schemaDetails[request.schemaPath]?.source || '';
    const text =
      request.kind === 'source'
        ? source
        : request.kind === 'descriptors'
          ? `demo descriptor bundle for ${request.schemaPath}`
          : `// demo generated ${request.language || 'rust'} artifact\n${source}`;
    return {
      revision: {
        name: `projects/${request.project}/repos/${request.repo}/revisions/6ddcd8c5b0dd`,
        project: request.project,
        repo: request.repo,
        commitId: '6ddcd8c5b0dd',
        resolvedFrom: request.ref,
      },
      content: new Blob([text], { type: 'text/plain' }),
      mediaType: 'text/plain',
      artifactDigest: 'sha256:demo-artifact',
      closureDigest: 'sha256:demo-closure',
    };
  }

  async listConflicts(
    _project: string,
    _repo: string,
    bookmark: string,
  ): Promise<ConflictList> {
    await wait();
    return {
      bookmark,
      conflicts: [{ schemaPath: 'order.proto', declarationName: 'Order' }],
    };
  }

  async renderConflict(
    _project: string,
    _repo: string,
    bookmark: string,
    schemaPath: string,
    declarationName: string,
  ): Promise<ConflictDetail> {
    await wait();
    return {
      bookmark,
      schemaPath,
      declarationName,
      rendered: '<<<<<<< left\nmessage Order { string id = 1; }\n=======\nmessage Order { bytes id = 1; }\n>>>>>>> right',
    };
  }

  async resolveConflict(
    _project: string,
    _repo: string,
    _request: ResolveConflictRequest,
  ): Promise<ResolveConflictResult> {
    await wait();
    return {
      commitId: 'demo-resolution-commit',
      changeId: 'demo-resolution-change',
      remainingConflicts: [],
    };
  }

  async listRepos(
    project: string,
    pageToken = '',
    pageSize = 50,
    namePrefix = '',
  ): Promise<RepoPage> {
    await wait();
    const names = project === 'acme' ? ['billing', 'commerce'] : ['events', 'schemas'];
    const repositories = names
      .filter((repo) => repo.startsWith(namePrefix))
      .map((repo) => ({
        project,
        repo,
        defaultBranch: 'main',
        protectedBranches: ['main', 'release/*'],
        compatibility: 'full' as const,
      }));
    const offset = Number.parseInt(pageToken || '0', 10) || 0;
    const effectivePageSize = Math.min(1, Math.max(1, pageSize));
    const end = Math.min(repositories.length, offset + effectivePageSize);
    return {
      repositories: repositories.slice(offset, end),
      nextPageToken: end < repositories.length ? end.toString() : '',
    };
  }

  async getRepo(project: string, repo: string): Promise<RepoSummary | undefined> {
    await wait();
    const names = project === 'acme' ? ['billing', 'commerce'] : ['events', 'schemas'];
    if (!names.includes(repo)) return undefined;
    return {
      project,
      repo,
      defaultBranch: 'main',
      protectedBranches: ['main', 'release/*'],
      compatibility: 'full',
    };
  }

  async getRepoDashboard(
    project: string,
    repo: string,
    _ref: string,
    pageToken = '',
    pageSize = 50,
  ): Promise<RepoDashboardPage> {
    await wait();
    const schemas = [
      {
        path: 'order.proto',
        format: 'protobuf' as const,
        declarations: 1,
        dependencies: 1,
        conflictCount: 0,
        lastCommit: '6ddcd8c5b0dd',
      },
      {
        path: 'build_record.fbs',
        format: 'flatbuffers' as const,
        declarations: 1,
        dependencies: 0,
        conflictCount: 0,
        lastCommit: '4398b6668712',
      },
      {
        path: 'commerce.yaml',
        format: 'openapi' as const,
        declarations: 1,
        dependencies: 0,
        conflictCount: 1,
        lastCommit: '37119ed84bba',
      },
    ].sort((left, right) => left.path.localeCompare(right.path));
    const branches = ['main', 'feature/shipping-note', 'feature/catalog-api'].sort();
    const tags = ['release-2026-06-05'];
    const offset = Number.parseInt(pageToken || '0', 10) || 0;
    const effectivePageSize = Math.min(1, Math.max(1, pageSize));
    const end = offset + effectivePageSize;
    const hasMore =
      end < schemas.length || end < branches.length || end < tags.length;
    return {
      repo: {
        project,
        repo,
        defaultBranch: 'main',
        protectedBranches: ['main', 'release/*'],
        compatibility: 'full',
      },
      schemas: schemas.slice(offset, end),
      branches: branches.slice(offset, end),
      tags: tags.slice(offset, end),
      latestCommit: commits[0],
      latestOperation: operations[0],
      openConflicts: 1,
      resolvedCommit: '6ddcd8c5b0dd',
      nextPageToken: hasMore ? end.toString() : '',
    };
  }

  async getSchemaDetail(
    _project: string,
    _repo: string,
    schemaPath: string,
    _ref: string,
  ): Promise<SchemaDetail> {
    await wait();
    return schemaDetails[schemaPath] ?? schemaDetails['order.proto'];
  }

  async previewCodegen(request: CodegenPreviewRequest): Promise<CodegenPreview> {
    await wait(180);
    if (request.schemaPath.endsWith('.yaml')) {
      return {
        content: 'OpenAPI server/client codegen is not implemented in SchemaHub v1.',
        isArchive: false,
        atCommit: '37119ed84bba',
      };
    }

    if (request.schemaPath.endsWith('.fbs')) {
      return {
        content: request.rustPluggableBuffer
          ? `// automatically generated by flatc-rs
pub mod __flatc_rs_runtime {
  pub use ::flatc_rs_runtime::{FlatBufferRead, SliceBuffer};
}

pub fn root_as_build_record_in<'a, B>(buf: &'a B) -> Result<acme::commerce::BuildRecord<'a, B>, ::flatbuffers::InvalidFlatbuffer>
where
  B: ?Sized + __flatc_rs_runtime::FlatBufferRead,
{
  let bytes = buf.all_bytes().ok_or(::flatbuffers::InvalidFlatbuffer::InconsistentUnion)?;
  ::flatbuffers::root::<acme::commerce::BuildRecord>(bytes)?;
  Ok(unsafe { acme::commerce::BuildRecord::init_from_buffer(buf, __flatc_rs_runtime::root_loc(buf)) })
}
`
          : `// automatically generated by flatc-rs
pub fn root_as_build_record(buf: &[u8]) -> Result<acme::commerce::BuildRecord<'_>, ::flatbuffers::InvalidFlatbuffer> {
  ::flatbuffers::root::<acme::commerce::BuildRecord>(buf)
}
`,
        isArchive: false,
        atCommit: '4398b6668712',
      };
    }

    return {
      content: `#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Order {
  #[prost(string, tag="1")]
  pub id: ::prost::alloc::string::String,
  #[prost(message, optional, tag="2")]
  pub total: ::core::option::Option<Money>,
  #[prost(string, tag="3")]
  pub shipping_note: ::prost::alloc::string::String,
}
`,
      isArchive: false,
      atCommit: '6ddcd8c5b0dd',
    };
  }

  async diff(
    _project: string,
    _repo: string,
    base: string,
    head: string,
    schemaPath?: string,
  ): Promise<DiffResult> {
    await wait();
    return {
      base,
      head,
      changes: [
        {
          schemaPath: schemaPath || 'order.proto',
          declaration: 'Order',
          kind: 'modified',
          compatibility: 'compatible',
          summary: 'Added optional string field shipping_note = 3',
        },
        {
          schemaPath: 'commerce.yaml',
          declaration: 'path:/orders/{id}',
          kind: 'modified',
          compatibility: 'warning',
          summary: 'Response schema ref changed; inspect downstream clients',
        },
      ],
    };
  }

  async listCommits(
    _project: string,
    _repo: string,
    _ref: string,
    limit: number,
  ): Promise<CommitEntry[]> {
    await wait();
    return commits.slice(0, limit);
  }

  async listOperations(
    _project: string,
    _repo: string,
    _ref: string,
    limit: number,
  ): Promise<OperationEntry[]> {
    await wait();
    return operations.slice(0, limit);
  }

  async getServerConfig(): Promise<ServerConfig> {
    await wait();
    return {
      storageBackend: 'redb',
      authMode: 'noop',
      maxOpsPerTransaction: 100,
      maxSchemasPerTransaction: 20,
      supportedFormats: ['protobuf', 'flatbuffers', 'openapi'],
    };
  }
}

export const schemaHubClient = new MockSchemaHubClient();
