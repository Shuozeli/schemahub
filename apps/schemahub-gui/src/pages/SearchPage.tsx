import {
  Alert,
  Badge,
  Button,
  Group,
  Paper,
  Stack,
  Table,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { FileCode2, FileSearch, GitCommit, GitPullRequest, Search } from 'lucide-react';
import { FormEvent, ReactNode, useEffect, useState } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';

import { useRepository, useSearchResources } from '../api/queries';
import type { SearchResourceKind, SearchResult } from '../api/types';
import { ChangeStatusBadge, RefBadge } from '../components/badges';

export function SearchPage() {
  const navigate = useNavigate();
  const { project = '', repo = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const query = searchParams.get('q') || '';
  const { data: repository } = useRepository(project, repo);
  const refName = searchParams.get('ref') || repository?.defaultBranch || '';
  const [draftQuery, setDraftQuery] = useState(query);
  const { data, error, isFetching } = useSearchResources(project, repo, query, refName);

  useEffect(() => setDraftQuery(query), [query]);

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const next = draftQuery.trim();
    if (!next || !refName) return;
    setSearchParams({ q: next, ref: refName });
  }

  return (
    <Stack>
      <div>
        <Text c="dimmed" size="xs" fw={700} tt="uppercase">
          {project} / {repo}
        </Text>
        <Group gap="xs">
          <Title order={2}>Repository search</Title>
          {refName ? <RefBadge refName={refName} /> : null}
        </Group>
        <Text size="sm" c="dimmed">
          Find schemas, declarations, immutable revisions, and durable change records.
        </Text>
      </div>

      <form onSubmit={submit}>
        <Group align="flex-end">
          <TextInput
            label="Search query"
            leftSection={<Search size={16} />}
            value={draftQuery}
            onChange={(event) => setDraftQuery(event.currentTarget.value)}
            placeholder="Schema, declaration, commit, actor, or change intent"
            style={{ flex: 1 }}
            autoFocus
          />
          <Button type="submit" loading={isFetching} disabled={!draftQuery.trim() || !refName}>
            Search
          </Button>
        </Group>
      </form>

      {error ? <Alert color="red">{error.message}</Alert> : null}

      <Paper withBorder radius="sm">
        <Table verticalSpacing="sm" highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Type</Table.Th>
              <Table.Th>Result</Table.Th>
              <Table.Th>Context</Table.Th>
              <Table.Th>Status</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {!query ? (
              <Table.Tr>
                <Table.Td colSpan={4}>Enter a query to search this repository.</Table.Td>
              </Table.Tr>
            ) : isFetching && !data ? (
              <Table.Tr>
                <Table.Td colSpan={4}>Searching...</Table.Td>
              </Table.Tr>
            ) : !data?.results.length ? (
              <Table.Tr>
                <Table.Td colSpan={4}>No matching resources at {refName}.</Table.Td>
              </Table.Tr>
            ) : (
              data.results.map((result, index) => (
                <Table.Tr
                  key={`${result.kind}-${result.title}-${index}`}
                  className="tableRowLink"
                  onClick={() => navigate(resultPath(project, repo, data.ref, result))}
                >
                  <Table.Td>
                    <Group gap="xs" wrap="nowrap">
                      {resultIcon(result.kind)}
                      <Badge variant="light" color={resultColor(result.kind)}>
                        {result.kind}
                      </Badge>
                    </Group>
                  </Table.Td>
                  <Table.Td>
                    <Text fw={600}>{result.title}</Text>
                    {result.declarationName ? (
                      <Text size="xs" c="dimmed" className="mono">
                        {result.declarationName}
                      </Text>
                    ) : null}
                  </Table.Td>
                  <Table.Td>
                    <Text size="sm">{result.description}</Text>
                    <Text size="xs" c="dimmed" className="mono">
                      {result.schemaPath || result.revision || result.changeId || ''}
                    </Text>
                  </Table.Td>
                  <Table.Td>
                    {result.status ? <ChangeStatusBadge status={result.status} /> : '-'}
                  </Table.Td>
                </Table.Tr>
              ))
            )}
          </Table.Tbody>
        </Table>
      </Paper>
    </Stack>
  );
}

function resultPath(project: string, repo: string, refName: string, result: SearchResult) {
  const base = `/projects/${encodeURIComponent(project)}/repos/${encodeURIComponent(repo)}`;
  if (result.kind === 'change' && result.changeId) {
    return `${base}/changes/${encodeURIComponent(result.changeId)}`;
  }
  if ((result.kind === 'schema' || result.kind === 'declaration') && result.schemaPath) {
    return `${base}/schemas/${encodePath(result.schemaPath)}?ref=${encodeURIComponent(refName)}`;
  }
  if (result.kind === 'revision' && result.revision) {
    return `${base}/history?ref=${encodeURIComponent(`@${result.revision}`)}`;
  }
  return base;
}

function encodePath(value: string) {
  return value.split('/').map(encodeURIComponent).join('/');
}

function resultIcon(kind: SearchResourceKind): ReactNode {
  if (kind === 'schema') return <FileCode2 size={16} />;
  if (kind === 'declaration') return <FileSearch size={16} />;
  if (kind === 'revision') return <GitCommit size={16} />;
  return <GitPullRequest size={16} />;
}

function resultColor(kind: SearchResourceKind) {
  if (kind === 'schema') return 'blue';
  if (kind === 'declaration') return 'cyan';
  if (kind === 'revision') return 'violet';
  return 'yellow';
}
