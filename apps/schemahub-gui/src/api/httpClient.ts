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

type HistoryResponse = {
  commits: CommitEntry[];
  operations: OperationEntry[];
};

export class HttpSchemaHubClient implements SchemaHubClient {
  constructor(private readonly baseUrl: string) {}

  async listProjects(): Promise<ProjectSummary[]> {
    return this.get('/api/projects');
  }

  async getRepoDashboard(project: string, repo: string, ref: string): Promise<RepoDashboard> {
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/dashboard?ref=${encode(ref)}`,
    );
  }

  async getSchemaDetail(
    project: string,
    repo: string,
    schemaPath: string,
    ref: string,
  ): Promise<SchemaDetail> {
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/schemas/${encodePath(
        schemaPath,
      )}?ref=${encode(ref)}`,
    );
  }

  async previewCodegen(request: CodegenPreviewRequest): Promise<CodegenPreview> {
    return this.post('/api/codegen/preview', request);
  }

  async diff(
    project: string,
    repo: string,
    base: string,
    head: string,
    schemaPath?: string,
  ): Promise<DiffResult> {
    const params = new URLSearchParams({ base, head });
    if (schemaPath) params.set('schemaPath', schemaPath);
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/diff?${params.toString()}`,
    );
  }

  async listCommits(
    project: string,
    repo: string,
    ref: string,
    limit: number,
  ): Promise<CommitEntry[]> {
    const history = await this.history(project, repo, ref, limit);
    return history.commits;
  }

  async listOperations(project: string, repo: string, limit: number): Promise<OperationEntry[]> {
    const history = await this.history(project, repo, 'main', limit);
    return history.operations;
  }

  async getServerConfig(): Promise<ServerConfig> {
    return this.get('/api/admin/config');
  }

  private async history(
    project: string,
    repo: string,
    ref: string,
    limit: number,
  ): Promise<HistoryResponse> {
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/history?ref=${encode(
        ref,
      )}&limit=${limit}`,
    );
  }

  private async get<T>(path: string): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`);
    return readJson<T>(response);
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    return readJson<T>(response);
  }
}

async function readJson<T>(response: Response): Promise<T> {
  if (response.ok) {
    return response.json() as Promise<T>;
  }
  let message = `${response.status} ${response.statusText}`;
  try {
    const body = (await response.json()) as { error?: string };
    if (body.error) message = body.error;
  } catch {
    // Keep the HTTP status message when the response is not JSON.
  }
  throw new Error(message);
}

function encode(value: string) {
  return encodeURIComponent(value);
}

function encodePath(value: string) {
  return value.split('/').map(encode).join('/');
}
