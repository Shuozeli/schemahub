import {
  Alert,
  Button,
  Group,
  Modal,
  Paper,
  Stack,
  Table,
  Text,
  Textarea,
  TextInput,
  Title,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import { Bot, Plus } from 'lucide-react';
import { FormEvent, useState } from 'react';
import { useNavigate, useParams } from 'react-router-dom';

import { useChanges, useCreateChange, useRepository } from '../api/queries';
import { ChangeStatusBadge, RefBadge } from '../components/badges';

export function ChangesPage() {
  const navigate = useNavigate();
  const { project = '', repo = '' } = useParams();
  const { data: repository } = useRepository(project, repo);
  const { data: changes = [], error, isLoading } = useChanges(project, repo);
  const create = useCreateChange(project, repo);
  const [opened, { open, close }] = useDisclosure(false);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [externalReferences, setExternalReferences] = useState('');

  const defaultBookmark = repository?.defaultBranch || '';

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const change = await create.mutateAsync({
      title,
      description,
      externalReferences: externalReferences
        .split('\n')
        .map((reference) => reference.trim())
        .filter(Boolean),
      targetBookmark: defaultBookmark,
    });
    close();
    setTitle('');
    setDescription('');
    setExternalReferences('');
    navigate(`${changePath(project, repo, change.name)}`);
  }

  return (
    <Stack>
      <Group justify="space-between" align="flex-start">
        <div>
          <Text c="dimmed" size="xs" fw={700} tt="uppercase">
            {project} / {repo}
          </Text>
          <Title order={2}>Change proposals</Title>
          <Text c="dimmed" size="sm">
            Durable schema intent shared by people, agents, the CLI, and the web console.
          </Text>
        </div>
        <Button leftSection={<Plus size={16} />} onClick={open} disabled={!defaultBookmark}>
          Record change note
        </Button>
      </Group>

      {error ? <Alert color="red">{error.message}</Alert> : null}

      <Paper withBorder radius="sm">
        <Table verticalSpacing="sm" highlightOnHover>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Proposal</Table.Th>
              <Table.Th>Status</Table.Th>
              <Table.Th>Actor</Table.Th>
              <Table.Th>Target</Table.Th>
              <Table.Th>Edits</Table.Th>
              <Table.Th>Updated</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {isLoading ? (
              <Table.Tr>
                <Table.Td colSpan={6}>Loading change proposals...</Table.Td>
              </Table.Tr>
            ) : changes.length === 0 ? (
              <Table.Tr>
                <Table.Td colSpan={6}>
                  No proposals yet. Record intent here or create an executable change with the CLI.
                </Table.Td>
              </Table.Tr>
            ) : (
              [...changes].reverse().map((change) => (
                <Table.Tr
                  key={change.name}
                  className="tableRowLink"
                  onClick={() => navigate(changePath(project, repo, change.name))}
                >
                  <Table.Td>
                    <Text fw={600}>{change.title}</Text>
                    <Text size="xs" c="dimmed" className="mono">
                      {changeId(change.name)}
                    </Text>
                  </Table.Td>
                  <Table.Td>
                    <ChangeStatusBadge status={change.status} />
                  </Table.Td>
                  <Table.Td>
                    <Group gap={6} wrap="nowrap">
                      {change.createdBy.kind === 'agent' ? <Bot size={15} /> : null}
                      <div>
                        <Text size="sm">
                          {change.createdBy.displayName || change.createdBy.identity}
                        </Text>
                        {change.createdBy.delegatedBy ? (
                          <Text size="xs" c="dimmed">
                            delegated by {change.createdBy.delegatedBy}
                          </Text>
                        ) : null}
                      </div>
                    </Group>
                  </Table.Td>
                  <Table.Td>
                    <RefBadge refName={change.targetBookmark} />
                  </Table.Td>
                  <Table.Td>{change.edits.length || 'note only'}</Table.Td>
                  <Table.Td>{formatTime(change.updateTimeUnixMs)}</Table.Td>
                </Table.Tr>
              ))
            )}
          </Table.Tbody>
        </Table>
      </Paper>

      <Modal opened={opened} onClose={close} title="Record schema-change intent" centered>
        <form onSubmit={submit}>
          <Stack>
            <Alert color="blue" icon={<Bot size={16} />}>
              The server attributes this note to the authenticated human or agent. A note remains a
              draft until an executable edit is attached and validated.
            </Alert>
            <TextInput
              label="Title"
              description="A concise storage-contract outcome"
              value={title}
              onChange={(event) => setTitle(event.currentTarget.value)}
              maxLength={200}
              required
              autoFocus
            />
            <Textarea
              label="Description"
              description="Why the schema should change, observations, and acceptance criteria"
              value={description}
              onChange={(event) => setDescription(event.currentTarget.value)}
              autosize
              minRows={4}
            />
            <Textarea
              label="External references"
              description="Optional issue, incident, design, or automation references; one per line"
              value={externalReferences}
              onChange={(event) => setExternalReferences(event.currentTarget.value)}
              autosize
              minRows={2}
            />
            <TextInput label="Target bookmark" value={defaultBookmark} readOnly />
            {create.error ? <Alert color="red">{create.error.message}</Alert> : null}
            <Group justify="flex-end">
              <Button variant="default" onClick={close}>
                Cancel
              </Button>
              <Button type="submit" loading={create.isPending} disabled={!title.trim()}>
                Record note
              </Button>
            </Group>
          </Stack>
        </form>
      </Modal>
    </Stack>
  );
}

function changeId(name: string) {
  const parts = name.split('/');
  return parts[parts.length - 1] || name;
}

function changePath(project: string, repo: string, name: string) {
  return `/projects/${encodeURIComponent(project)}/repos/${encodeURIComponent(repo)}/changes/${encodeURIComponent(changeId(name))}`;
}

function formatTime(unixMs: number) {
  return new Date(unixMs).toLocaleString();
}
