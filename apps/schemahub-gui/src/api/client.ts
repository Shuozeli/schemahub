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

export interface SchemaHubClient {
  listProjects(): Promise<ProjectSummary[]>;
  getRepoDashboard(project: string, repo: string, ref: string): Promise<RepoDashboard>;
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
  listOperations(project: string, repo: string, limit: number): Promise<OperationEntry[]>;
  getServerConfig(): Promise<ServerConfig>;
}

