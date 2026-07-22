import { Group, Select, Stack, Text, Title } from '@mantine/core';
import { GitBranch } from 'lucide-react';
import { RefBadge } from './badges';

type ResourceHeaderProps = {
  eyebrow: string;
  title: string;
  subtitle?: string;
  refName: string;
  refs?: string[];
  onRefChange: (value: string) => void;
};

export function ResourceHeader({
  eyebrow,
  title,
  subtitle,
  refName,
  refs = [],
  onRefChange,
}: ResourceHeaderProps) {
  const refOptions = [...new Set([refName, ...refs])].map((value) => ({ value, label: value }));
  return (
    <Group justify="space-between" align="flex-start" mb="md">
      <Stack gap={4}>
        <Text size="xs" fw={700} c="dimmed">
          {eyebrow}
        </Text>
        <Group gap="xs">
          <Title order={2}>{title}</Title>
          <RefBadge refName={refName} />
        </Group>
        {subtitle ? (
          <Text size="sm" c="dimmed">
            {subtitle}
          </Text>
        ) : null}
      </Stack>
      <Select
        leftSection={<GitBranch size={16} />}
        aria-label="Active ref"
        value={refName}
        onChange={(value) => value && onRefChange(value)}
        data={refOptions}
        w={280}
      />
    </Group>
  );
}
