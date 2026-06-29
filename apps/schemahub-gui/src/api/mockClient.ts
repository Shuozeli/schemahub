import type {
  CodegenPreview,
  CodegenPreviewRequest,
  CommitEntry,
  DiffResult,
  OperationEntry,
  ProjectSummary,
  RepoDashboard,
  SchemaDetail,
  ServerConfig,
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
  async listProjects(): Promise<ProjectSummary[]> {
    await wait();
    return [
      {
        name: 'acme',
        visibility: 'public',
        role: 'Owner',
        repos: 2,
        lastOperation: 'MergeBranch',
        lastActivity: '2026-06-24T04:14:21Z',
      },
      {
        name: 'platform',
        visibility: 'private',
        role: 'Maintainer',
        repos: 4,
        lastOperation: 'CreateTag',
        lastActivity: '2026-06-23T18:41:00Z',
      },
    ];
  }

  async getRepoDashboard(project: string, repo: string, _ref: string): Promise<RepoDashboard> {
    await wait();
    return {
      repo: {
        project,
        repo,
        defaultBranch: 'main',
        protectedBranches: ['main', 'release/*'],
        compatibility: 'full',
      },
      schemas: [
        {
          path: 'order.proto',
          format: 'protobuf',
          declarations: 1,
          dependencies: 1,
          conflictCount: 0,
          lastCommit: '6ddcd8c5b0dd',
        },
        {
          path: 'build_record.fbs',
          format: 'flatbuffers',
          declarations: 1,
          dependencies: 0,
          conflictCount: 0,
          lastCommit: '4398b6668712',
        },
        {
          path: 'commerce.yaml',
          format: 'openapi',
          declarations: 1,
          dependencies: 0,
          conflictCount: 1,
          lastCommit: '37119ed84bba',
        },
      ],
      branches: ['main', 'feature/shipping-note', 'feature/catalog-api'],
      tags: ['release-2026-06-05'],
      latestCommit: commits[0],
      latestOperation: operations[0],
      openConflicts: 1,
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

  async listOperations(_project: string, _repo: string, limit: number): Promise<OperationEntry[]> {
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
