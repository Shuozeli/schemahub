import { Badge, Group, Paper, Stack, Table, Tabs, Text, Title } from '@mantine/core';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { Boxes, GitBranch, GitCommit, GitPullRequest, ShieldCheck } from 'lucide-react';
import type { ReactNode } from 'react';

import { useRepoDashboard } from '../api/queries';
import { FormatBadge } from '../components/badges';
import { ResourceHeader } from '../components/ResourceHeader';

function MetricCell({
  label,
  value,
  icon,
}: {
  label: string;
  value: string | number;
  icon: ReactNode;
}) {
  return (
    <div className="metricCell">
      <Group gap="xs" c="dimmed">
        {icon}
        <Text size="xs" fw={700}>
          {label}
        </Text>
      </Group>
      <Text mt={4} size="lg" fw={700}>
        {value}
      </Text>
    </div>
  );
}

export function RepoDashboardPage() {
  const navigate = useNavigate();
  const { project = 'acme', repo = 'commerce' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const refName = searchParams.get('ref') || 'main';
  const { data, isLoading } = useRepoDashboard(project, repo, refName);

  if (isLoading || !data) {
    return <Text>Loading repo dashboard...</Text>;
  }

  return (
    <Stack>
      <ResourceHeader
        eyebrow={`${project} / ${repo}`}
        title="Repo dashboard"
        subtitle="Schema inventory, refs, protection policy, and recent audit activity."
        refName={refName}
        onRefChange={(value) => setSearchParams({ ref: value })}
      />

      <div className="metricStrip">
        <MetricCell label="Schemas" value={data.schemas.length} icon={<Boxes size={16} />} />
        <MetricCell label="Branches" value={data.branches.length} icon={<GitBranch size={16} />} />
        <MetricCell label="Tags" value={data.tags.length} icon={<GitCommit size={16} />} />
        <MetricCell
          label="Open conflicts"
          value={data.openConflicts}
          icon={<GitPullRequest size={16} />}
        />
        <MetricCell
          label="Compatibility"
          value={data.repo.compatibility}
          icon={<ShieldCheck size={16} />}
        />
      </div>

      <Tabs defaultValue="schemas">
        <Tabs.List>
          <Tabs.Tab value="schemas">Schemas</Tabs.Tab>
          <Tabs.Tab value="activity">Activity</Tabs.Tab>
          <Tabs.Tab value="refs">Refs</Tabs.Tab>
        </Tabs.List>

        <Tabs.Panel value="schemas" pt="md">
          <Paper withBorder radius="sm">
            <Table verticalSpacing="sm" highlightOnHover>
              <Table.Thead>
                <Table.Tr>
                  <Table.Th>Schema</Table.Th>
                  <Table.Th>Format</Table.Th>
                  <Table.Th>Declarations</Table.Th>
                  <Table.Th>Dependencies</Table.Th>
                  <Table.Th>Conflicts</Table.Th>
                  <Table.Th>Last commit</Table.Th>
                </Table.Tr>
              </Table.Thead>
              <Table.Tbody>
                {data.schemas.map((schema) => (
                  <Table.Tr
                    key={schema.path}
                    className="tableRowLink"
                    onClick={() =>
                      navigate(
                        `/projects/${project}/repos/${repo}/schemas/${schema.path}?ref=${encodeURIComponent(refName)}`,
                      )
                    }
                  >
                    <Table.Td fw={600}>{schema.path}</Table.Td>
                    <Table.Td>
                      <FormatBadge format={schema.format} />
                    </Table.Td>
                    <Table.Td>{schema.declarations}</Table.Td>
                    <Table.Td>{schema.dependencies}</Table.Td>
                    <Table.Td>
                      <Badge color={schema.conflictCount ? 'yellow' : 'gray'} variant="light">
                        {schema.conflictCount}
                      </Badge>
                    </Table.Td>
                    <Table.Td className="mono">{schema.lastCommit}</Table.Td>
                  </Table.Tr>
                ))}
              </Table.Tbody>
            </Table>
          </Paper>
        </Tabs.Panel>

        <Tabs.Panel value="activity" pt="md">
          <Group grow align="stretch">
            <Paper withBorder p="md" radius="sm">
              <Title order={4}>Latest commit</Title>
              <Text mt="sm" fw={600}>
                {data.latestCommit.message}
              </Text>
              <Text size="sm" c="dimmed" className="mono">
                {data.latestCommit.commit}
              </Text>
            </Paper>
            <Paper withBorder p="md" radius="sm">
              <Title order={4}>Latest operation</Title>
              <Text mt="sm" fw={600}>
                {data.latestOperation.action}
              </Text>
              <Text size="sm" c="dimmed">
                {data.latestOperation.target}
              </Text>
            </Paper>
          </Group>
        </Tabs.Panel>

        <Tabs.Panel value="refs" pt="md">
          <Group align="flex-start">
            <Paper withBorder p="md" radius="sm" miw={280}>
              <Title order={4}>Branches</Title>
              <Stack mt="sm" gap="xs">
                {data.branches.map((branch) => (
                  <Badge key={branch} variant="light" color="blue" className="mono">
                    {branch}
                  </Badge>
                ))}
              </Stack>
            </Paper>
            <Paper withBorder p="md" radius="sm" miw={280}>
              <Title order={4}>Tags</Title>
              <Stack mt="sm" gap="xs">
                {data.tags.map((tag) => (
                  <Badge key={tag} variant="light" color="violet" className="mono">
                    tag:{tag}
                  </Badge>
                ))}
              </Stack>
            </Paper>
          </Group>
        </Tabs.Panel>
      </Tabs>
    </Stack>
  );
}
