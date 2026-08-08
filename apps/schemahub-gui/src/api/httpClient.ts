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
  SchemaRevision,
  ServerConfig,
  SessionInfo,
  UpdateChangeEditsRequest,
} from './types';
import type { SchemaHubClient } from './client';

type HistoryResponse = {
  commits: CommitEntry[];
  operations: OperationEntry[];
};

export class HttpSchemaHubClient implements SchemaHubClient {
  constructor(
    private readonly baseUrl: string,
    private readonly token: () => string | undefined = () => undefined,
  ) {}

  async listProjects(pageToken = '', pageSize = 50): Promise<ProjectPage> {
    const params = new URLSearchParams({ pageSize: pageSize.toString() });
    if (pageToken) params.set('pageToken', pageToken);
    return this.get(`/api/projects?${params.toString()}`);
  }

  async listRepos(
    project: string,
    pageToken = '',
    pageSize = 50,
    namePrefix = '',
  ): Promise<RepoPage> {
    const params = new URLSearchParams({ pageSize: pageSize.toString() });
    if (pageToken) params.set('pageToken', pageToken);
    if (namePrefix) params.set('namePrefix', namePrefix);
    return this.get(`/api/projects/${encode(project)}/repos?${params.toString()}`);
  }

  async getRepo(project: string, repo: string): Promise<RepoSummary | undefined> {
    const page = await this.listRepos(project, '', 1, repo);
    return page.repositories.find((repository) => repository.repo === repo);
  }

  async getRepoDashboard(
    project: string,
    repo: string,
    ref: string,
    pageToken = '',
    pageSize = 50,
  ): Promise<RepoDashboardPage> {
    const params = new URLSearchParams({ ref, pageSize: pageSize.toString() });
    if (pageToken) params.set('pageToken', pageToken);
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/dashboard?${params.toString()}`,
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

  async listOperations(
    project: string,
    repo: string,
    ref: string,
    limit: number,
  ): Promise<OperationEntry[]> {
    const history = await this.history(project, repo, ref, limit);
    return history.operations;
  }

  async getServerConfig(): Promise<ServerConfig> {
    return this.get('/api/admin/config');
  }

  async getSession(): Promise<SessionInfo> {
    return this.get('/api/session');
  }

  async listChanges(
    project: string,
    repo: string,
    pageToken = '',
    pageSize = 50,
    status = '',
  ): Promise<ChangePage> {
    const params = new URLSearchParams({ pageSize: pageSize.toString() });
    if (pageToken) params.set('pageToken', pageToken);
    if (status) params.set('status', status);
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/changes?${params.toString()}`,
    );
  }

  async getChange(project: string, repo: string, changeId: string): Promise<ChangeRecord> {
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/changes/${encode(changeId)}`,
    );
  }

  async createChange(
    project: string,
    repo: string,
    request: CreateChangeRequest,
  ): Promise<ChangeRecord> {
    return this.post(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/changes`,
      request,
    );
  }

  async updateChangeEdits(
    project: string,
    repo: string,
    changeId: string,
    request: UpdateChangeEditsRequest,
  ): Promise<ChangeRecord> {
    return this.patch(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/changes/${encode(changeId)}`,
      request,
    );
  }

  async changeAction(
    project: string,
    repo: string,
    changeId: string,
    action: ChangeAction,
    request: ChangeActionRequest,
  ): Promise<ChangeRecord> {
    return this.post(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/changes/${encode(
        changeId,
      )}/actions/${encode(action)}`,
      request,
    );
  }

  async search(
    project: string,
    repo: string,
    query: string,
    ref: string,
    limit = 50,
  ): Promise<SearchResponse> {
    const params = new URLSearchParams({ q: query, ref, limit: String(limit) });
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/search?${params.toString()}`,
    );
  }

  async downloadArtifact(request: ArtifactDownloadRequest): Promise<ArtifactDownload> {
    const revision = await this.resolveRevision(request.project, request.repo, request.ref);
    const params = new URLSearchParams({ kind: request.kind });
    if (request.language) params.set('language', request.language);
    if (request.rustPluggableBuffer) params.set('rustPluggableBuffer', 'true');
    const response = await fetch(
      `${this.baseUrl}/api/projects/${encode(request.project)}/repos/${encode(
        request.repo,
      )}/revisions/${encode(revision.commitId)}/artifacts/${encodePath(
        request.schemaPath,
      )}?${params.toString()}`,
      { headers: this.authHeaders() },
    );
    if (!response.ok) return readJson<never>(response);
    return {
      revision,
      content: await response.blob(),
      mediaType: response.headers.get('content-type') || 'application/octet-stream',
      artifactDigest: unquote(response.headers.get('etag') || ''),
      closureDigest: response.headers.get('x-schemahub-closure-digest') || '',
    };
  }

  async listConflicts(project: string, repo: string, bookmark: string): Promise<ConflictList> {
    const params = new URLSearchParams({ bookmark });
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/conflicts?${params.toString()}`,
    );
  }

  async renderConflict(
    project: string,
    repo: string,
    bookmark: string,
    schemaPath: string,
    declarationName: string,
  ): Promise<ConflictDetail> {
    const params = new URLSearchParams({ bookmark, schemaPath, declarationName });
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/conflicts/render?${params.toString()}`,
    );
  }

  async resolveConflict(
    project: string,
    repo: string,
    request: ResolveConflictRequest,
  ): Promise<ResolveConflictResult> {
    return this.post(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/conflicts/resolve`,
      request,
    );
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
    const response = await fetch(`${this.baseUrl}${path}`, {
      headers: this.authHeaders(),
    });
    return readJson<T>(response);
  }

  private async resolveRevision(
    project: string,
    repo: string,
    ref: string,
  ): Promise<SchemaRevision> {
    return this.get(
      `/api/projects/${encode(project)}/repos/${encode(repo)}/revisions/resolve?ref=${encode(ref)}`,
    );
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...this.authHeaders() },
      body: JSON.stringify(body),
    });
    return readJson<T>(response);
  }

  private async patch<T>(path: string, body: unknown): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method: 'PATCH',
      headers: { 'content-type': 'application/json', ...this.authHeaders() },
      body: JSON.stringify(body),
    });
    return readJson<T>(response);
  }

  private authHeaders(): Record<string, string> {
    const token = this.token()?.trim();
    return token ? { authorization: `Bearer ${token}` } : {};
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

function unquote(value: string) {
  return value.startsWith('"') && value.endsWith('"') ? value.slice(1, -1) : value;
}
