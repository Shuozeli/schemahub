import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { schemaHubClient } from './index';
import type {
  ChangeAction,
  ChangeActionRequest,
  CreateChangeRequest,
  RepoDashboard,
  ResolveConflictRequest,
  UpdateChangeEditsRequest,
} from './types';

export function useProjects() {
  return useInfiniteQuery({
    queryKey: ['projects'],
    queryFn: ({ pageParam }) => schemaHubClient.listProjects(pageParam, 50),
    initialPageParam: '',
    getNextPageParam: (page) => page.nextPageToken || undefined,
  });
}

export function useRepos(project: string) {
  return useInfiniteQuery({
    queryKey: ['repos', project],
    queryFn: ({ pageParam }) => schemaHubClient.listRepos(project, pageParam, 50),
    initialPageParam: '',
    getNextPageParam: (page) => page.nextPageToken || undefined,
    enabled: project.length > 0,
  });
}

export function useRepository(project: string, repo: string) {
  return useQuery({
    queryKey: ['repo', project, repo],
    queryFn: () => schemaHubClient.getRepo(project, repo),
    enabled: project.length > 0 && repo.length > 0,
  });
}

export function useRepoDashboard(project: string, repo: string, ref: string) {
  return useInfiniteQuery({
    queryKey: ['repo-dashboard', project, repo, ref],
    queryFn: ({ pageParam }) =>
      schemaHubClient.getRepoDashboard(project, repo, ref, pageParam, 50),
    initialPageParam: '',
    getNextPageParam: (page) => page.nextPageToken || undefined,
    select: (result): RepoDashboard => {
      const first = result.pages[0];
      if (!first) throw new Error('repository dashboard returned no pages');
      return {
        repo: first.repo,
        schemas: result.pages.flatMap((page) => page.schemas),
        branches: result.pages.flatMap((page) => page.branches),
        tags: result.pages.flatMap((page) => page.tags),
        latestCommit: first.latestCommit,
        latestOperation: first.latestOperation,
        openConflicts: first.openConflicts,
        resolvedCommit: first.resolvedCommit,
      };
    },
    enabled: project.length > 0 && repo.length > 0 && ref.length > 0,
  });
}

export function useSchemaDetail(project: string, repo: string, schemaPath: string, ref: string) {
  return useQuery({
    queryKey: ['schema-detail', project, repo, schemaPath, ref],
    queryFn: () => schemaHubClient.getSchemaDetail(project, repo, schemaPath, ref),
    enabled:
      project.length > 0 && repo.length > 0 && schemaPath.length > 0 && ref.length > 0,
  });
}

export function useDiff(
  project: string,
  repo: string,
  base: string,
  head: string,
  schemaPath?: string,
) {
  return useQuery({
    queryKey: ['diff', project, repo, base, head, schemaPath],
    queryFn: () => schemaHubClient.diff(project, repo, base, head, schemaPath),
    enabled: project.length > 0 && repo.length > 0 && base.length > 0 && head.length > 0,
  });
}

export function useHistory(project: string, repo: string, ref: string) {
  return useQuery({
    queryKey: ['history', project, repo, ref],
    queryFn: async () => ({
      commits: await schemaHubClient.listCommits(project, repo, ref, 25),
      operations: await schemaHubClient.listOperations(project, repo, ref, 50),
    }),
    enabled: project.length > 0 && repo.length > 0 && ref.length > 0,
  });
}

export function useServerConfig() {
  return useQuery({
    queryKey: ['server-config'],
    queryFn: () => schemaHubClient.getServerConfig(),
  });
}

export function useSession() {
  return useQuery({
    queryKey: ['session'],
    queryFn: () => schemaHubClient.getSession(),
    staleTime: 0,
  });
}

export function useChanges(project: string, repo: string) {
  return useInfiniteQuery({
    queryKey: ['changes', project, repo],
    queryFn: ({ pageParam }) => schemaHubClient.listChanges(project, repo, pageParam, 50),
    initialPageParam: '',
    getNextPageParam: (page) => page.nextPageToken || undefined,
    select: (result) => result.pages.flatMap((page) => page.changes),
    enabled: project.length > 0 && repo.length > 0,
  });
}

export function useChange(project: string, repo: string, changeId: string) {
  return useQuery({
    queryKey: ['change', project, repo, changeId],
    queryFn: () => schemaHubClient.getChange(project, repo, changeId),
    enabled: project.length > 0 && repo.length > 0 && changeId.length > 0,
  });
}

export function useCreateChange(project: string, repo: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: CreateChangeRequest) =>
      schemaHubClient.createChange(project, repo, request),
    onSuccess: (change) => {
      queryClient.setQueryData(['change', project, repo, changeId(change.name)], change);
      void queryClient.invalidateQueries({ queryKey: ['changes', project, repo] });
    },
  });
}

export function useUpdateChangeEdits(
  project: string,
  repo: string,
  changeIdValue: string,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: UpdateChangeEditsRequest) =>
      schemaHubClient.updateChangeEdits(project, repo, changeIdValue, request),
    onSuccess: (change) => {
      queryClient.setQueryData(['change', project, repo, changeIdValue], change);
      void queryClient.invalidateQueries({ queryKey: ['changes', project, repo] });
    },
  });
}

export function useChangeAction(project: string, repo: string, changeIdValue: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      action,
      request,
    }: {
      action: ChangeAction;
      request: ChangeActionRequest;
    }) => schemaHubClient.changeAction(project, repo, changeIdValue, action, request),
    onSuccess: (change) => {
      queryClient.setQueryData(['change', project, repo, changeIdValue], change);
      void queryClient.invalidateQueries({ queryKey: ['changes', project, repo] });
      if (change.status === 'applied') {
        void queryClient.invalidateQueries({ queryKey: ['repo-dashboard', project, repo] });
        void queryClient.invalidateQueries({ queryKey: ['history', project, repo] });
      }
    },
  });
}

export function useSearchResources(
  project: string,
  repo: string,
  query: string,
  ref: string,
) {
  return useQuery({
    queryKey: ['search', project, repo, query, ref],
    queryFn: () => schemaHubClient.search(project, repo, query, ref),
    enabled:
      project.length > 0 && repo.length > 0 && query.trim().length > 0 && ref.length > 0,
  });
}

export function useConflicts(project: string, repo: string, bookmark: string) {
  return useQuery({
    queryKey: ['conflicts', project, repo, bookmark],
    queryFn: () => schemaHubClient.listConflicts(project, repo, bookmark),
    enabled: project.length > 0 && repo.length > 0 && bookmark.length > 0,
  });
}

export function useConflict(
  project: string,
  repo: string,
  bookmark: string,
  schemaPath: string,
  declarationName: string,
) {
  return useQuery({
    queryKey: ['conflict', project, repo, bookmark, schemaPath, declarationName],
    queryFn: () =>
      schemaHubClient.renderConflict(project, repo, bookmark, schemaPath, declarationName),
    enabled:
      project.length > 0 &&
      repo.length > 0 &&
      bookmark.length > 0 &&
      schemaPath.length > 0 &&
      declarationName.length > 0,
  });
}

export function useResolveConflict(project: string, repo: string, bookmark: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (request: ResolveConflictRequest) =>
      schemaHubClient.resolveConflict(project, repo, request),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['conflicts', project, repo, bookmark] });
      void queryClient.invalidateQueries({ queryKey: ['conflict', project, repo, bookmark] });
      void queryClient.invalidateQueries({ queryKey: ['repo-dashboard', project, repo] });
      void queryClient.invalidateQueries({ queryKey: ['history', project, repo] });
    },
  });
}

function changeId(name: string) {
  const parts = name.split('/');
  return parts[parts.length - 1] || name;
}
