import { Badge, Group, Paper, SimpleGrid, Stack, Text, Title } from '@mantine/core';

import { useServerConfig } from '../api/queries';

export function AdminPage() {
  const { data, isLoading } = useServerConfig();

  if (isLoading || !data) {
    return <Text>Loading server config...</Text>;
  }

  return (
    <Stack>
      <div>
        <Title order={2}>Admin</Title>
        <Text c="dimmed" size="sm">
          Read-only server configuration and capability surface.
        </Text>
      </div>

      <SimpleGrid cols={{ base: 1, md: 2 }}>
        <Paper withBorder p="md" radius="sm">
          <Text size="xs" c="dimmed" fw={700}>
            Storage backend
          </Text>
          <Title order={3}>{data.storageBackend}</Title>
        </Paper>
        <Paper withBorder p="md" radius="sm">
          <Text size="xs" c="dimmed" fw={700}>
            Auth mode
          </Text>
          <Title order={3}>{data.authMode}</Title>
        </Paper>
        <Paper withBorder p="md" radius="sm">
          <Text size="xs" c="dimmed" fw={700}>
            Transaction limits
          </Text>
          <Text mt="sm">Max ops: {data.maxOpsPerTransaction}</Text>
          <Text>Max schemas: {data.maxSchemasPerTransaction}</Text>
        </Paper>
        <Paper withBorder p="md" radius="sm">
          <Text size="xs" c="dimmed" fw={700}>
            Supported formats
          </Text>
          <Group mt="sm">
            {data.supportedFormats.map((format) => (
              <Badge key={format} variant="light">
                {format}
              </Badge>
            ))}
          </Group>
        </Paper>
      </SimpleGrid>
    </Stack>
  );
}

