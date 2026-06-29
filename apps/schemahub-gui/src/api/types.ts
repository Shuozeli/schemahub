export type SchemaFormat = 'protobuf' | 'flatbuffers' | 'openapi';
export type CompatibilityState = 'compatible' | 'warning' | 'breaking' | 'unknown';

export type ProjectSummary = {
  name: string;
  visibility: 'public' | 'private';
  role: 'Owner' | 'Maintainer' | 'Writer' | 'Reader';
  repos: number;
  lastOperation: string;
  lastActivity: string;
};

export type RepoSummary = {
  project: string;
  repo: string;
  defaultBranch: string;
  protectedBranches: string[];
  compatibility: 'backward' | 'forward' | 'full' | 'disabled';
};

export type SchemaSummary = {
  path: string;
  format: SchemaFormat;
  declarations: number;
  dependencies: number;
  conflictCount: number;
  lastCommit: string;
};

export type RepoDashboard = {
  repo: RepoSummary;
  schemas: SchemaSummary[];
  branches: string[];
  tags: string[];
  latestCommit: CommitEntry;
  latestOperation: OperationEntry;
  openConflicts: number;
};

export type DeclarationSummary = {
  name: string;
  kind: 'message' | 'enum' | 'service' | 'table' | 'struct' | 'union' | 'path' | 'schema';
  detail: string;
  refs: string[];
};

export type Dependency = {
  importingSchema: string;
  importPath: string;
  resolvedCommit: string;
  status: 'resolved' | 'missing';
};

export type SchemaDetail = {
  path: string;
  format: SchemaFormat;
  source: string;
  declarations: DeclarationSummary[];
  dependencies: Dependency[];
};

export type CodegenPreviewRequest = {
  project: string;
  repo: string;
  schemaPath: string;
  ref: string;
  language: 'rust' | 'typescript';
  rustPluggableBuffer?: boolean;
};

export type CodegenPreview = {
  content: string;
  isArchive: boolean;
  atCommit: string;
};

export type DiffChange = {
  schemaPath: string;
  declaration: string;
  kind: 'added' | 'modified' | 'removed';
  compatibility: CompatibilityState;
  summary: string;
};

export type DiffResult = {
  base: string;
  head: string;
  changes: DiffChange[];
};

export type CommitEntry = {
  commit: string;
  changeId: string;
  parents: string[];
  author: string;
  message: string;
  timestamp: string;
};

export type OperationEntry = {
  opId: string;
  author: string;
  action: string;
  target: string;
  before?: string;
  after?: string;
  timestamp: string;
};

export type ServerConfig = {
  storageBackend: 'redb' | 'postgres';
  authMode: 'noop' | 'bearer-rbac';
  maxOpsPerTransaction: number;
  maxSchemasPerTransaction: number;
  supportedFormats: SchemaFormat[];
};

