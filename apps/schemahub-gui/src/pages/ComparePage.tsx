import { Group, Paper, Select, Stack, Table, Text, Title } from '@mantine/core';
import { useParams, useSearchParams } from 'react-router-dom';

import { useDiff } from '../api/queries';
import { CompatibilityBadge } from '../components/badges';
import { CodeViewer } from '../components/CodeViewer';
import { ResourceHeader } from '../components/ResourceHeader';

const refs = [
  { value: 'tag:release-2026-06-05', label: 'tag:release-2026-06-05' },
  { value: 'main', label: 'main' },
  { value: 'feature/shipping-note', label: 'feature/shipping-note' },
];

export function ComparePage() {
  const { project = 'acme', repo = 'commerce' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const refName = searchParams.get('ref') || 'main';
  const base = searchParams.get('base') || 'tag:release-2026-06-05';
  const head = searchParams.get('head') || 'main';
  const schema = searchParams.get('schema') || '';
  const { data, isLoading } = useDiff(project, repo, base, head, schema || undefined);

  const updateParam = (key: string, value: string) => {
    const next = new URLSearchParams(searchParams);
    next.set(key, value);
    setSearchParams(next);
  };

  return (
    <Stack>
      <ResourceHeader
        eyebrow={`${project} / ${repo}`}
        title="Compare"
        subtitle="Review schema changes between refs before merge or release."
        refName={refName}
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
            data={[
              { value: '', label: 'All schemas' },
              { value: 'order.proto', label: 'order.proto' },
              { value: 'commerce.yaml', label: 'commerce.yaml' },
            ]}
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
          value={`--- ${base}/order.proto
+++ ${head}/order.proto
@@ message Order
   string id = 1;
   Money total = 2;
+  string shipping_note = 3;
`}
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

