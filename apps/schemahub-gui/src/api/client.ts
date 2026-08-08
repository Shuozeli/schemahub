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

export interface SchemaHubClient {
  listProjects(pageToken?: string, pageSize?: number): Promise<ProjectPage>;
  listRepos(
    project: string,
    pageToken?: string,
    pageSize?: number,
    namePrefix?: string,
  ): Promise<RepoPage>;
  getRepo(project: string, repo: string): Promise<RepoSummary | undefined>;
  getRepoDashboard(
    project: string,
    repo: string,
    ref: string,
    pageToken?: string,
    pageSize?: number,
  ): Promise<RepoDashboardPage>;
  getSchemaDetail(
    project: string,
    repo: string,
    schemaPath: string,
    ref: string,
  ): Promise<SchemaDetail>;
  previewCodegen(request: CodegenPreviewRequest): Promise<CodegenPreview>;
  diff(
    project: string,
    repo: string,
    base: string,
    head: string,
    schemaPath?: string,
  ): Promise<DiffResult>;
  listCommits(project: string, repo: string, ref: string, limit: number): Promise<CommitEntry[]>;
  listOperations(
    project: string,
    repo: string,
    ref: string,
    limit: number,
  ): Promise<OperationEntry[]>;
  getServerConfig(): Promise<ServerConfig>;
  getSession(): Promise<SessionInfo>;
  listChanges(
    project: string,
    repo: string,
    pageToken?: string,
    pageSize?: number,
    status?: string,
  ): Promise<ChangePage>;
  getChange(project: string, repo: string, changeId: string): Promise<ChangeRecord>;
  createChange(
    project: string,
    repo: string,
    request: CreateChangeRequest,
  ): Promise<ChangeRecord>;
  updateChangeEdits(
    project: string,
    repo: string,
    changeId: string,
    request: UpdateChangeEditsRequest,
  ): Promise<ChangeRecord>;
  changeAction(
    project: string,
    repo: string,
    changeId: string,
    action: ChangeAction,
    request: ChangeActionRequest,
  ): Promise<ChangeRecord>;
  search(
    project: string,
    repo: string,
    query: string,
    ref: string,
    limit?: number,
  ): Promise<SearchResponse>;
  downloadArtifact(request: ArtifactDownloadRequest): Promise<ArtifactDownload>;
  listConflicts(project: string, repo: string, bookmark: string): Promise<ConflictList>;
  renderConflict(
    project: string,
    repo: string,
    bookmark: string,
    schemaPath: string,
    declarationName: string,
  ): Promise<ConflictDetail>;
  resolveConflict(
    project: string,
    repo: string,
    request: ResolveConflictRequest,
  ): Promise<ResolveConflictResult>;
}
