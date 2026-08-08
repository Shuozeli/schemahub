import { Alert, Paper, Stack, Table, Tabs, Text } from '@mantine/core';
import { useParams, useSearchParams } from 'react-router-dom';

import { useHistory, useRepoDashboard, useRepository } from '../api/queries';
import { ResourceHeader } from '../components/ResourceHeader';

export function HistoryPage() {
  const { project = '', repo = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const { data: repository } = useRepository(project, repo);
  const refName = searchParams.get('ref') || repository?.defaultBranch || '';
  const { data, error, isLoading } = useHistory(project, repo, refName);
  const {
    data: dashboard,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
  } = useRepoDashboard(project, repo, refName);

  if (error) {
    return <Alert color="red">{error.message}</Alert>;
  }

  return (
    <Stack>
      <ResourceHeader
        eyebrow={`${project} / ${repo}`}
        title="History"
        subtitle="Content commits and JJ-style operation audit log."
        refName={refName}
        refs={[
          ...(dashboard?.branches ?? []),
          ...(dashboard?.tags ?? []).map((tag) => `tag:${tag}`),
        ]}
        onRefChange={(value) => setSearchParams({ ref: value })}
        hasMoreRefs={hasNextPage}
        loadingMoreRefs={isFetchingNextPage}
        onLoadMoreRefs={() => void fetchNextPage()}
      />

      <Tabs defaultValue="commits">
        <Tabs.List>
          <Tabs.Tab value="commits">Commits</Tabs.Tab>
          <Tabs.Tab value="operations">Operations</Tabs.Tab>
        </Tabs.List>

        <Tabs.Panel value="commits" pt="md">
          <Paper withBorder radius="sm">
            <Table verticalSpacing="sm">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Commit</Table.Th>
                  <Table.Th>Change ID</Table.Th>
                  <Table.Th>Author</Table.Th>
                  <Table.Th>Message</Table.Th>
                  <Table.Th>Timestamp</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {isLoading || !data ? (
                  <Table.Tr>
                    <Table.Td colSpan={5}>Loading commits...</Table.Td>
                  </Table.Tr>
                ) : (
                  data.commits.map((commit) => (
                    <Table.Tr key={commit.commit}>
                      <Table.Td className="mono">{commit.commit}</Table.Td>
                      <Table.Td className="mono">{commit.changeId}</Table.Td>
                      <Table.Td>{commit.author}</Table.Td>
                      <Table.Td>{commit.message}</Table.Td>
                      <Table.Td className="mono">{commit.timestamp}</Table.Td>
                    </Table.Tr>
                  ))
                )}
              </Table.Tbody>
            </Table>
          </Paper>
        </Tabs.Panel>

        <Tabs.Panel value="operations" pt="md">
          <Paper withBorder radius="sm">
            <Table verticalSpacing="sm">
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Operation</Table.Th>
                  <Table.Th>Action</Table.Th>
                  <Table.Th>Target</Table.Th>
                  <Table.Th>Author</Table.Th>
                  <Table.Th>After</Table.Th>
                  <Table.Th>Timestamp</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {data?.operations.map((op) => (
                  <Table.Tr key={op.opId}>
                    <Table.Td className="mono">{op.opId}</Table.Td>
                    <Table.Td>{op.action}</Table.Td>
                    <Table.Td>{op.target}</Table.Td>
                    <Table.Td>{op.author}</Table.Td>
                    <Table.Td className="mono">{op.after || '-'}</Table.Td>
                    <Table.Td className="mono">{op.timestamp}</Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Paper>
          <Text mt="xs" size="sm" c="dimmed">
            Commits describe content state. Operations describe registry actions that changed the view.
          </Text>
        </Tabs.Panel>
      </Tabs>
    </Stack>
  );
}
