import { Badge, Group, Paper, Stack, Table, Text, Title } from '@mantine/core';
import { useNavigate, useParams } from 'react-router-dom';

import { useRepos } from '../api/queries';

export function ProjectPage() {
  const navigate = useNavigate();
  const { project = '' } = useParams();
  const { data = [], error, isLoading } = useRepos(project);

  return (
    <Stack>
      <Group justify="space-between">
        <div>
          <Text c="dimmed" size="xs" fw={700} tt="uppercase">
            Project
          </Text>
          <Title order={2}>{project}</Title>
          <Text c="dimmed" size="sm">
            Select a persisted repository to inspect schemas, changes, refs, and artifacts.
          </Text>
        </div>
      </Group>
      <Paper withBorder radius="sm">
        <Table verticalSpacing="sm" highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Repository</Table.Th>
              <Table.Th>Default branch</Table.Th>
              <Table.Th>Compatibility</Table.Th>
              <Table.Th>Protected branches</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {isLoading ? (
              <Table.Tr>
                <Table.Td colSpan={4}>Loading repositories...</Table.Td>
              </Table.Tr>
            ) : error ? (
              <Table.Tr>
                <Table.Td colSpan={4} c="red">
                  {error.message}
                </Table.Td>
              </Table.Tr>
            ) : data.length === 0 ? (
              <Table.Tr>
                <Table.Td colSpan={4}>No active repositories in this project.</Table.Td>
              </Table.Tr>
            ) : (
              data.map((repository) => (
                <Table.Tr
                  key={repository.repo}
                  className="tableRowLink"
                  onClick={() =>
                    navigate(`/projects/${repository.project}/repos/${repository.repo}`)
                  }
                >
                  <Table.Td fw={600}>{repository.repo}</Table.Td>
                  <Table.Td className="mono">{repository.defaultBranch}</Table.Td>
                  <Table.Td>
                    <Badge variant="light">{repository.compatibility}</Badge>
                  </Table.Td>
                  <Table.Td>{repository.protectedBranches.join(', ') || 'None'}</Table.Td>
                </Table.Tr>
              ))
            )}
          </Table.Tbody>
        </Table>
      </Paper>
    </Stack>
  );
}
