import { Alert, List, Paper, Stack, Table, Tabs, Text, Title } from '@mantine/core';
import { useParams, useSearchParams } from 'react-router-dom';

import { useRepoDashboard, useRepository, useSchemaDetail } from '../api/queries';
import { FormatBadge } from '../components/badges';
import { CodegenPreviewPanel } from '../components/CodegenPreview';
import { CodeViewer } from '../components/CodeViewer';
import { ResourceHeader } from '../components/ResourceHeader';

function languageForPath(path: string) {
  if (path.endsWith('.proto')) return 'protobuf';
  if (path.endsWith('.fbs')) return 'fbs';
  return 'yaml';
}

export function SchemaDetailPage() {
  const { project = '', repo = '', '*': wildcard } = useParams();
  const schemaPath = wildcard || '';
  const [searchParams, setSearchParams] = useSearchParams();
  const { data: repository } = useRepository(project, repo);
  const refName = searchParams.get('ref') || repository?.defaultBranch || '';
  const { data, error, isLoading } = useSchemaDetail(project, repo, schemaPath, refName);
  const { data: dashboard } = useRepoDashboard(project, repo, refName);

  if (error) {
    return <Alert color="red">{error.message}</Alert>;
  }

  if (isLoading || !data) {
    return <Text>Loading schema...</Text>;
  }

  return (
    <Stack>
      <ResourceHeader
        eyebrow={`${project} / ${repo}`}
        title={data.path}
        subtitle="Canonical source, declarations, dependencies, and codegen preview."
        refName={refName}
        refs={[
          ...(dashboard?.branches ?? []),
          ...(dashboard?.tags ?? []).map((tag) => `tag:${tag}`),
        ]}
        onRefChange={(value) => setSearchParams({ ref: value })}
      />

      <Tabs defaultValue="source">
        <Tabs.List>
          <Tabs.Tab value="source">Source</Tabs.Tab>
          <Tabs.Tab value="declarations">Declarations</Tabs.Tab>
          <Tabs.Tab value="dependencies">Dependencies</Tabs.Tab>
          <Tabs.Tab value="codegen">Codegen</Tabs.Tab>
        </Tabs.List>

        <Tabs.Panel value="source" pt="md">
          <div className="splitGrid">
            <Paper withBorder p="md" radius="sm">
              <Title order={4}>Declarations</Title>
              <List mt="sm" spacing="xs">
                {data.declarations.map((decl) => (
                  <List.Item key={decl.name}>
                    <Text fw={600}>{decl.name}</Text>
                    <Text size="xs" c="dimmed">
                      {decl.kind}
                    </Text>
                  </List.Item>
                ))}
              </List>
            </Paper>
            <CodeViewer value={data.source} language={languageForPath(data.path)} height={520} />
            <Paper withBorder p="md" radius="sm">
              <Title order={4}>Schema</Title>
              <Stack mt="sm" gap="sm">
                <div>
                  <Text size="xs" c="dimmed" fw={700}>
                    Format
                  </Text>
                  <FormatBadge format={data.format} />
                </div>
                <div>
                  <Text size="xs" c="dimmed" fw={700}>
                    Current ref
                  </Text>
                  <Text className="mono">{refName}</Text>
                </div>
                <div>
                  <Text size="xs" c="dimmed" fw={700}>
                    Dependencies
                  </Text>
                  <Text>{data.dependencies.length}</Text>
                </div>
              </Stack>
            </Paper>
          </div>
        </Tabs.Panel>

        <Tabs.Panel value="declarations" pt="md">
          <Paper withBorder radius="sm">
            <Table verticalSpacing="sm">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Name</Table.Th>
                  <Table.Th>Kind</Table.Th>
                  <Table.Th>Summary</Table.Th>
                  <Table.Th>Refs</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {data.declarations.map((decl) => (
                  <Table.Tr key={decl.name}>
                    <Table.Td fw={600}>{decl.name}</Table.Td>
                    <Table.Td>{decl.kind}</Table.Td>
                    <Table.Td>{decl.detail}</Table.Td>
                    <Table.Td>{decl.refs.join(', ') || '-'}</Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Paper>
        </Tabs.Panel>

        <Tabs.Panel value="dependencies" pt="md">
          <Paper withBorder radius="sm">
            <Table verticalSpacing="sm">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Importing schema</Table.Th>
                  <Table.Th>Import path</Table.Th>
                  <Table.Th>Resolved commit</Table.Th>
                  <Table.Th>Status</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {data.dependencies.length ? (
                  data.dependencies.map((dep) => (
                    <Table.Tr key={dep.importPath}>
                      <Table.Td>{dep.importingSchema}</Table.Td>
                      <Table.Td>{dep.importPath}</Table.Td>
                      <Table.Td className="mono">{dep.resolvedCommit}</Table.Td>
                      <Table.Td>{dep.status}</Table.Td>
                    </Table.Tr>
                  ))
                ) : (
                  <Table.Tr>
                    <Table.Td colSpan={4}>No imports for this schema.</Table.Td>
                  </Table.Tr>
                )}
              </Table.Tbody>
            </Table>
          </Paper>
        </Tabs.Panel>

        <Tabs.Panel value="codegen" pt="md">
          <CodegenPreviewPanel
            project={project}
            repo={repo}
            schemaPath={schemaPath}
            format={data.format}
            refName={refName}
          />
        </Tabs.Panel>
      </Tabs>
    </Stack>
  );
}
