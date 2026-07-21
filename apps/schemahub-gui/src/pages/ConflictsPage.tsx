import {
  Alert,
  Badge,
  Button,
  Group,
  Paper,
  Select,
  Stack,
  Text,
  Textarea,
  TextInput,
  Title,
} from '@mantine/core';
import { CheckCircle2, GitMerge, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useParams, useSearchParams } from 'react-router-dom';

import {
  useConflict,
  useConflicts,
  useRepoDashboard,
  useRepository,
  useResolveConflict,
} from '../api/queries';
import { CodeViewer } from '../components/CodeViewer';

export function ConflictsPage() {
  const { project = '', repo = '' } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const { data: repository } = useRepository(project, repo);
  const bookmark = searchParams.get('bookmark') || repository?.defaultBranch || '';
  const { data: dashboard } = useRepoDashboard(project, repo, bookmark);
  const { data: conflictList, error: listError, isLoading } = useConflicts(
    project,
    repo,
    bookmark,
  );
  const [selectedKey, setSelectedKey] = useState('');
  const selected = useMemo(
    () =>
      conflictList?.conflicts.find(
        (conflict) => conflictKey(conflict.schemaPath, conflict.declarationName) === selectedKey,
      ),
    [conflictList, selectedKey],
  );
  const { data: detail, error: detailError } = useConflict(
    project,
    repo,
    bookmark,
    selected?.schemaPath || '',
    selected?.declarationName || '',
  );
  const resolve = useResolveConflict(project, repo, bookmark);
  const [resolvedSource, setResolvedSource] = useState('');
  const [message, setMessage] = useState('');

  useEffect(() => {
    if (!selectedKey && conflictList?.conflicts[0]) {
      const first = conflictList.conflicts[0];
      setSelectedKey(conflictKey(first.schemaPath, first.declarationName));
    }
  }, [conflictList, selectedKey]);

  useEffect(() => {
    setResolvedSource('');
    setMessage('');
    resolve.reset();
    // Reset only when the selected declaration changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedKey]);

  async function submitResolution() {
    if (!selected) return;
    try {
      await resolve.mutateAsync({
        bookmark,
        schemaPath: selected.schemaPath,
        declarationName: selected.declarationName,
        resolvedSource,
        message,
      });
      setResolvedSource('');
    } catch {
      // The mutation retains and renders the server's structured failure.
    }
  }

  const branches = dashboard?.branches.length
    ? dashboard.branches.map((branch) => ({ value: branch, label: branch }))
    : bookmark
      ? [{ value: bookmark, label: bookmark }]
      : [];

  return (
    <Stack>
      <Group justify="space-between" align="flex-start">
        <div>
          <Text c="dimmed" size="xs" fw={700} tt="uppercase">
            {project} / {repo}
          </Text>
          <Group gap="xs">
            <Title order={2}>Conflict resolution</Title>
            <Badge color={conflictList?.conflicts.length ? 'yellow' : 'green'} variant="light">
              {conflictList?.conflicts.length || 0} open
            </Badge>
          </Group>
          <Text c="dimmed" size="sm">
            Inspect competing declaration sides and commit a compiler-validated resolution.
          </Text>
        </div>
        <Select
          label="Bookmark"
          value={bookmark}
          data={branches}
          onChange={(value) => value && setSearchParams({ bookmark: value })}
          w={260}
        />
      </Group>

      {listError ? <Alert color="red">{listError.message}</Alert> : null}
      {resolve.data ? (
        <Alert color="green" title="Resolution committed">
          Commit <span className="mono">{resolve.data.commitId}</span>, change{' '}
          <span className="mono">{resolve.data.changeId}</span>.
        </Alert>
      ) : null}
      {isLoading ? <Text>Loading conflicts...</Text> : null}
      {!isLoading && conflictList?.conflicts.length === 0 ? (
        <Alert color="green" icon={<CheckCircle2 size={18} />} title="Bookmark is clean">
          No unresolved declarations remain on {bookmark}.
        </Alert>
      ) : null}

      {conflictList?.conflicts.length ? (
        <div className="conflictGrid">
          <Paper withBorder p="sm">
            <Stack gap="xs">
              {conflictList.conflicts.map((conflict) => {
                const key = conflictKey(conflict.schemaPath, conflict.declarationName);
                return (
                  <Button
                    key={key}
                    variant={selectedKey === key ? 'light' : 'subtle'}
                    color="yellow"
                    justify="flex-start"
                    leftSection={<TriangleAlert size={16} />}
                    onClick={() => setSelectedKey(key)}
                    fullWidth
                  >
                    <div style={{ textAlign: 'left' }}>
                      <Text size="sm" fw={600}>
                        {conflict.declarationName}
                      </Text>
                      <Text size="xs" c="dimmed" className="mono">
                        {conflict.schemaPath}
                      </Text>
                    </div>
                  </Button>
                );
              })}
            </Stack>
          </Paper>

          <Stack>
            {detailError ? <Alert color="red">{detailError.message}</Alert> : null}
            <div>
              <Text fw={700}>Competing sides</Text>
              <Text size="xs" c="dimmed">
                Server-rendered conflict for {selected?.schemaPath} :: {selected?.declarationName}
              </Text>
            </div>
            <CodeViewer
              value={detail?.rendered || 'Loading the conflict rendering...'}
              language="diff"
              height={520}
            />
          </Stack>

          <Paper withBorder p="md">
            <Stack>
              <div>
                <Group gap="xs">
                  <GitMerge size={18} />
                  <Title order={4}>Proposed resolution</Title>
                </Group>
                <Text size="sm" c="dimmed" mt={4}>
                  Paste the complete schema source containing the resolved declaration. The server
                  parses it, extracts the selected declaration, and validates it before commit.
                </Text>
              </div>
              <Textarea
                label="Resolved schema source"
                value={resolvedSource}
                onChange={(event) => setResolvedSource(event.currentTarget.value)}
                autosize
                minRows={16}
                className="mono"
              />
              <TextInput
                label="Commit message"
                value={message}
                onChange={(event) => setMessage(event.currentTarget.value)}
                placeholder={`Resolve ${selected?.declarationName || 'declaration'} conflict`}
              />
              {resolve.error ? (
                <Alert color="red" title="Resolution rejected">
                  {resolve.error.message}
                </Alert>
              ) : null}
              <Button
                onClick={() => void submitResolution()}
                loading={resolve.isPending}
                disabled={!selected || !resolvedSource.trim()}
              >
                Validate and resolve
              </Button>
            </Stack>
          </Paper>
        </div>
      ) : null}
    </Stack>
  );
}

function conflictKey(schemaPath: string, declarationName: string) {
  return `${schemaPath}\u0000${declarationName}`;
}
