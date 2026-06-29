import { useQuery } from '@tanstack/react-query';

import { schemaHubClient } from './mockClient';

export function useProjects() {
  return useQuery({
    queryKey: ['projects'],
    queryFn: () => schemaHubClient.listProjects(),
  });
}

export function useRepoDashboard(project: string, repo: string, ref: string) {
  return useQuery({
    queryKey: ['repo-dashboard', project, repo, ref],
    queryFn: () => schemaHubClient.getRepoDashboard(project, repo, ref),
  });
}

export function useSchemaDetail(project: string, repo: string, schemaPath: string, ref: string) {
  return useQuery({
    queryKey: ['schema-detail', project, repo, schemaPath, ref],
    queryFn: () => schemaHubClient.getSchemaDetail(project, repo, schemaPath, ref),
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
  });
}

export function useHistory(project: string, repo: string, ref: string) {
  return useQuery({
    queryKey: ['history', project, repo, ref],
    queryFn: async () => ({
      commits: await schemaHubClient.listCommits(project, repo, ref, 25),
      operations: await schemaHubClient.listOperations(project, repo, 50),
    }),
  });
}

export function useServerConfig() {
  return useQuery({
    queryKey: ['server-config'],
    queryFn: () => schemaHubClient.getServerConfig(),
  });
}

