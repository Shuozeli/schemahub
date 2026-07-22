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

export type SessionInfo = {
  authenticated: boolean;
  id?: string;
  display?: string;
  kind: 'anonymous' | 'human' | 'agent' | 'service';
  delegatedBy?: string;
};

export type ChangeStatus =
  | 'draft'
  | 'ready'
  | 'applying'
  | 'applied'
  | 'rejected'
  | 'abandoned';

export type ChangeActor = {
  identity: string;
  kind: 'anonymous' | 'human' | 'agent' | 'service';
  displayName?: string;
  delegatedBy?: string;
};

export type ChangeEdit = {
  kind: 'mutation' | 'replace_source' | 'delete_schema';
  schemaPath: string;
  formatId: string;
};

export type ChangeValidationIssue = {
  code: string;
  message: string;
  schemaName?: string;
  declarationName?: string;
};

export type ChangeValidation = {
  valid: boolean;
  resolvedBaseCommit: string;
  editDigest: string;
  issues: ChangeValidationIssue[];
  validatedAtUnixMs: number;
  validatorVersion: string;
};

export type ChangeReview = {
  reviewer: ChangeActor;
  decision: 'approved' | 'rejected';
  reason: string;
  createTimeUnixMs: number;
};

export type ChangeApplyResult = {
  commitId: string;
  changeId: string;
  operationId: string;
  conflictedDeclarations: string[];
  artifactDigest?: string;
};

export type ChangeRecord = {
  name: string;
  project: string;
  repo: string;
  targetBookmark: string;
  baseRevision?: string;
  title: string;
  description: string;
  externalReferences: string[];
  edits: ChangeEdit[];
  createdBy: ChangeActor;
  status: ChangeStatus;
  validation?: ChangeValidation;
  reviews: ChangeReview[];
  applyResult?: ChangeApplyResult;
  etag: string;
  createTimeUnixMs: number;
  updateTimeUnixMs: number;
};

export type CreateChangeRequest = {
  title: string;
  description: string;
  externalReferences: string[];
  targetBookmark: string;
  baseRevision?: string;
  changeId?: string;
};

export type ChangeAction = 'validate' | 'ready' | 'approve' | 'reject' | 'apply' | 'abandon';

export type ChangeActionRequest = {
  etag: string;
  reason?: string;
  requestId?: string;
};

export type SearchResourceKind = 'schema' | 'declaration' | 'revision' | 'change';

export type SearchResult = {
  kind: SearchResourceKind;
  title: string;
  description: string;
  schemaPath?: string;
  declarationName?: string;
  revision?: string;
  changeId?: string;
  status?: ChangeStatus;
};

export type SearchResponse = {
  query: string;
  ref: string;
  results: SearchResult[];
};

export type SchemaRevision = {
  name: string;
  project: string;
  repo: string;
  commitId: string;
  resolvedFrom: string;
};

export type ArtifactKind = 'source' | 'descriptors' | 'generated-code';

export type ArtifactDownloadRequest = {
  project: string;
  repo: string;
  schemaPath: string;
  ref: string;
  kind: ArtifactKind;
  language?: 'rust' | 'typescript';
  rustPluggableBuffer?: boolean;
};

export type ArtifactDownload = {
  revision: SchemaRevision;
  content: Blob;
  mediaType: string;
  artifactDigest: string;
  closureDigest: string;
};

export type ConflictSummary = {
  schemaPath: string;
  declarationName: string;
};

export type ConflictList = {
  bookmark: string;
  conflicts: ConflictSummary[];
};

export type ConflictDetail = ConflictSummary & {
  bookmark: string;
  rendered: string;
};

export type ResolveConflictRequest = ConflictSummary & {
  bookmark: string;
  resolvedSource: string;
  message?: string;
};

export type ResolveConflictResult = {
  commitId: string;
  changeId: string;
  remainingConflicts: string[];
};
