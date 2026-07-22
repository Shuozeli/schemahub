import { Alert, Group, Paper, Select, Stack, Table, Text, Title } from '@mantine/core';
import { useParams, useSearchParams } from 'react-router-dom';

import { useDiff, useRepoDashboard, useRepository } from '../api/queries';
import { CompatibilityBadge } from '../components/badges';
import { CodeViewer } from '../components/CodeViewer';
import { ResourceHeader } from '../components/ResourceHeader';

export function ComparePage() {
  const { project = '', repo = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const { data: repository } = useRepository(project, repo);
  const defaultRef = repository?.defaultBranch || '';
  const base = searchParams.get('base') || defaultRef;
  const head = searchParams.get('head') || defaultRef;
  const refName = searchParams.get('ref') || head;
  const schema = searchParams.get('schema') || '';
  const { data, error, isLoading } = useDiff(project, repo, base, head, schema || undefined);
  const { data: dashboard } = useRepoDashboard(project, repo, refName);
  const refs = [
    ...(dashboard?.branches ?? []).map((value) => ({ value, label: value })),
    ...(dashboard?.tags ?? []).map((tag) => ({ value: `tag:${tag}`, label: `tag:${tag}` })),
  ];
  const schemas = [
    { value: '', label: 'All schemas' },
    ...(dashboard?.schemas ?? []).map(({ path }) => ({ value: path, label: path })),
  ];

  const updateParam = (key: string, value: string) => {
    const next = new URLSearchParams(searchParams);
    next.set(key, value);
    setSearchParams(next);
  };

  if (error) {
    return <Alert color="red">{error.message}</Alert>;
  }

  return (
    <Stack>
      <ResourceHeader
        eyebrow={`${project} / ${repo}`}
        title="Compare"
        subtitle="Review schema changes between refs before merge or release."
        refName={refName}
        refs={refs.map(({ value }) => value)}
        onRefChange={(value) => updateParam('ref', value)}
      />

      <Paper withBorder p="md" radius="sm">
        <Group>
          <Select label="Base" value={base} onChange={(v) => v && updateParam('base', v)} data={refs} />
          <Select label="Head" value={head} onChange={(v) => v && updateParam('head', v)} data={refs} />
          <Select
            label="Schema"
            value={schema}
            onChange={(v) => updateParam('schema', v || '')}
            data={schemas}
          />
        </Group>
      </Paper>

      <div className="compareGrid">
        <Paper withBorder radius="sm">
          <Table verticalSpacing="sm">
            <Table.Thead>
              <Table.Tr>
                <Table.Th>Change</Table.Th>
                <Table.Th>State</Table.Th>
              </Table.Tr>
            </Table.Thead>
            <Table.Tbody>
              {isLoading || !data ? (
                <Table.Tr>
                  <Table.Td colSpan={2}>Loading diff...</Table.Td>
                </Table.Tr>
              ) : (
                data.changes.map((change) => (
                  <Table.Tr key={`${change.schemaPath}:${change.declaration}`}>
                    <Table.Td>
                      <Text fw={600}>{change.declaration}</Text>
                      <Text size="xs" c="dimmed">
                        {change.kind} in {change.schemaPath}
                      </Text>
                    </Table.Td>
                    <Table.Td>
                      <CompatibilityBadge state={change.compatibility} />
                    </Table.Td>
                  </Table.Tr>
                ))
              )}
            </Table.Tbody>
          </Table>
        </Paper>

        <CodeViewer
          language="diff"
          height={520}
          value={
            data?.changes
              .map(
                (change) =>
                  `${change.kind.toUpperCase()} ${change.schemaPath} :: ${change.declaration}\n${change.summary}`,
              )
              .join('\n\n') || `No declaration changes between ${base} and ${head}.`
          }
        />

        <Paper withBorder p="md" radius="sm">
          <Title order={4}>Compatibility</Title>
          <Stack mt="md" gap="sm">
            {data?.changes.map((change) => (
              <Paper key={change.summary} withBorder p="sm" radius="sm">
                <Group justify="space-between">
                  <Text fw={600}>{change.declaration}</Text>
                  <CompatibilityBadge state={change.compatibility} />
                </Group>
                <Text mt={4} size="sm" c="dimmed">
                  {change.summary}
                </Text>
              </Paper>
            ))}
          </Stack>
        </Paper>
      </div>
    </Stack>
  );
}
