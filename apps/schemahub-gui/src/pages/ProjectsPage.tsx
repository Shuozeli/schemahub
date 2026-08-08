import { Badge, Button, Group, Paper, Stack, Table, Text, Title } from '@mantine/core';
import { useNavigate } from 'react-router-dom';

import { useProjects } from '../api/queries';

export function ProjectsPage() {
  const navigate = useNavigate();
  const {
    data,
    error,
    fetchNextPage,
    hasNextPage,
    isFetchingNextPage,
    isLoading,
  } = useProjects();
  const projects = data?.pages.flatMap((page) => page.projects) ?? [];

  return (
    <Stack>
      <Group justify="space-between">
        <div>
          <Title order={2}>Projects</Title>
          <Text c="dimmed" size="sm">
            Pick a SchemaHub project to inspect repos, schemas, history, and codegen.
          </Text>
        </div>
      </Group>
      <Paper withBorder radius="sm">
        <Table verticalSpacing="sm" highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Project</Table.Th>
              <Table.Th>Visibility</Table.Th>
              <Table.Th>My role</Table.Th>
              <Table.Th>Last operation</Table.Th>
              <Table.Th>Last activity</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {isLoading ? (
              <Table.Tr>
                <Table.Td colSpan={5}>Loading projects...</Table.Td>
              </Table.Tr>
            ) : error ? (
              <Table.Tr>
                <Table.Td colSpan={5} c="red">
                  {error.message}
                </Table.Td>
              </Table.Tr>
            ) : projects.length === 0 ? (
              <Table.Tr>
                <Table.Td colSpan={5}>No projects are visible to this identity.</Table.Td>
              </Table.Tr>
            ) : (
              projects.map((project) => (
                <Table.Tr
                  key={project.name}
                  className="tableRowLink"
                  onClick={() => navigate(`/projects/${project.name}`)}
                >
                  <Table.Td fw={600}>{project.name}</Table.Td>
                  <Table.Td>
                    <Badge variant="light" color={project.visibility === 'public' ? 'green' : 'gray'}>
                      {project.visibility}
                    </Badge>
                  </Table.Td>
                  <Table.Td>{project.role}</Table.Td>
                  <Table.Td>{project.lastOperation}</Table.Td>
                  <Table.Td className="mono">{project.lastActivity}</Table.Td>
                </Table.Tr>
              ))
            )}
          </Table.Tbody>
        </Table>
      </Paper>
      {hasNextPage ? (
        <Button
          variant="default"
          loading={isFetchingNextPage}
          onClick={() => void fetchNextPage()}
        >
          Load more projects
        </Button>
      ) : null}
    </Stack>
  );
}
