import {
  Alert,
  Badge,
  Button,
  Code,
  Divider,
  Group,
  Paper,
  SimpleGrid,
  Stack,
  Table,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { Bot, CheckCircle2, CircleAlert, FilePenLine, GitCommit } from 'lucide-react';
import { useState } from 'react';
import { Link, useParams } from 'react-router-dom';

import { useChange, useChangeAction, useSession } from '../api/queries';
import type { ChangeAction, ChangeApplyResult, ChangeRecord } from '../api/types';
import { ChangeStatusBadge, RefBadge } from '../components/badges';

export function ChangeDetailPage() {
  const { project = '', repo = '', changeId = '' } = useParams();
  const { data: change, error, isLoading } = useChange(project, repo, changeId);
  const { data: session } = useSession();
  const action = useChangeAction(project, repo, changeId);
  const [reason, setReason] = useState('');

  if (isLoading) return <Text>Loading change proposal...</Text>;
  if (error || !change) {
    return <Alert color="red">{error?.message || 'Change proposal was not found.'}</Alert>;
  }

  const authorLabel = change.createdBy.displayName || change.createdBy.identity;
  const currentEtag = change.etag;
  const reviewedByCaller = change.reviews.some(
    (review) => review.reviewer.identity === session?.id,
  );
  const canReview =
    Boolean(session?.id) && session?.id !== change.createdBy.identity && !reviewedByCaller;

  async function run(nextAction: ChangeAction) {
    try {
      await action.mutateAsync({
        action: nextAction,
        request: {
          etag: currentEtag,
          reason,
          requestId: nextAction === 'apply' ? `gui-apply-${changeId}` : undefined,
        },
      });
      if (nextAction === 'approve' || nextAction === 'reject') setReason('');
    } catch {
      // The mutation retains the server error and renders it below.
    }
  }

  return (
    <Stack>
      <Group justify="space-between" align="flex-start">
        <div>
          <Text c="dimmed" size="xs" fw={700} tt="uppercase">
            {project} / {repo} / change {changeId}
          </Text>
          <Group gap="xs">
            <Title order={2}>{change.title}</Title>
            <ChangeStatusBadge status={change.status} />
          </Group>
          <Text c="dimmed" size="sm" mt={4}>
            {change.description || 'No description was recorded.'}
          </Text>
        </div>
        <Button
          component={Link}
          to={`/projects/${encodeURIComponent(project)}/repos/${encodeURIComponent(repo)}/changes`}
          variant="default"
        >
          All proposals
        </Button>
      </Group>

      {change.externalReferences.length > 0 ? (
        <Paper withBorder p="md">
          <Text size="xs" c="dimmed" fw={700} tt="uppercase">
            External references
          </Text>
          <Stack gap={4} mt="xs">
            {change.externalReferences.map((reference) => (
              <Code key={reference} block>
                {reference}
              </Code>
            ))}
          </Stack>
        </Paper>
      ) : null}

      <SimpleGrid cols={{ base: 1, md: 3 }} spacing="sm">
        <Paper withBorder p="md">
          <Text size="xs" c="dimmed" fw={700} tt="uppercase">
            Created by
          </Text>
          <Group mt="xs" gap="xs">
            {change.createdBy.kind === 'agent' ? <Bot size={18} /> : null}
            <div>
              <Text fw={600}>{authorLabel}</Text>
              <Text size="xs" c="dimmed">
                {change.createdBy.kind}
                {change.createdBy.delegatedBy
                  ? ` · delegated by ${change.createdBy.delegatedBy}`
                  : ''}
              </Text>
            </div>
          </Group>
        </Paper>
        <Paper withBorder p="md">
          <Text size="xs" c="dimmed" fw={700} tt="uppercase">
            Target
          </Text>
          <Group mt="xs">
            <RefBadge refName={change.targetBookmark} />
            {change.baseRevision ? <Code>{change.baseRevision}</Code> : <Text size="sm">latest</Text>}
          </Group>
        </Paper>
        <Paper withBorder p="md">
          <Text size="xs" c="dimmed" fw={700} tt="uppercase">
            Concurrency token
          </Text>
          <Code mt="xs">{change.etag}</Code>
          <Text size="xs" c="dimmed" mt="xs">
            Updated {new Date(change.updateTimeUnixMs).toLocaleString()}
          </Text>
        </Paper>
      </SimpleGrid>

      {change.edits.length === 0 ? (
        <Alert color="blue" icon={<FilePenLine size={18} />} title="Intent-only draft">
          This record preserves schema-change intent but has no executable edits. Attach mutations
          or source edits through the CLI or ChangeService before marking it ready.
        </Alert>
      ) : null}

      {action.error ? (
        <Alert color="red" icon={<CircleAlert size={18} />} title="Action was not accepted">
          {action.error.message} Refreshing the proposal will retrieve the current ETag if another
          actor changed it.
        </Alert>
      ) : null}

      <Paper withBorder p="md">
        <Group justify="space-between" align="flex-start">
          <div>
            <Title order={4}>Lifecycle actions</Title>
            <Text size="sm" c="dimmed">
              Every action is authorized and checked against the displayed ETag by the server.
            </Text>
          </div>
          <Group>
            {change.status === 'draft' ? (
              <>
                <Button
                  variant="light"
                  onClick={() => void run('validate')}
                  loading={action.isPending}
                >
                  Validate
                </Button>
                <Button
                  onClick={() => void run('ready')}
                  loading={action.isPending}
                  disabled={!change.validation?.valid || change.edits.length === 0}
                >
                  Mark ready
                </Button>
              </>
            ) : null}
            {change.status === 'ready' ? (
              <Button onClick={() => void run('apply')} loading={action.isPending} color="green">
                Apply safely
              </Button>
            ) : null}
            {change.status === 'applying' ? (
              <Button onClick={() => void run('apply')} loading={action.isPending}>
                Reconcile apply
              </Button>
            ) : null}
            {change.status === 'draft' || change.status === 'ready' ? (
              <Button
                variant="subtle"
                color="gray"
                onClick={() => void run('abandon')}
                loading={action.isPending}
              >
                Abandon
              </Button>
            ) : null}
          </Group>
        </Group>

        {change.status === 'ready' ? (
          <>
            <Divider my="md" />
            <TextInput
              label="Review reason"
              description={
                canReview
                  ? 'Approval reasons are optional; rejection reasons are required.'
                  : 'Authors cannot review their own proposal, and each reviewer acts once.'
              }
              value={reason}
              onChange={(event) => setReason(event.currentTarget.value)}
              disabled={!canReview}
            />
            <Group mt="sm">
              <Button
                variant="light"
                color="green"
                onClick={() => void run('approve')}
                disabled={!canReview}
                loading={action.isPending}
              >
                Approve
              </Button>
              <Button
                variant="light"
                color="red"
                onClick={() => void run('reject')}
                disabled={!canReview || !reason.trim()}
                loading={action.isPending}
              >
                Reject
              </Button>
            </Group>
          </>
        ) : null}
      </Paper>

      <Paper withBorder radius="sm">
        <Group p="md" justify="space-between">
          <Title order={4}>Executable edits</Title>
          <Badge variant="light">{change.edits.length}</Badge>
        </Group>
        <Table verticalSpacing="sm">
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Kind</Table.Th>
              <Table.Th>Schema</Table.Th>
              <Table.Th>Format</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {change.edits.length === 0 ? (
              <Table.Tr>
                <Table.Td colSpan={3}>No executable edits attached.</Table.Td>
              </Table.Tr>
            ) : (
              change.edits.map((edit, index) => (
                <Table.Tr key={`${edit.kind}-${edit.schemaPath}-${index}`}>
                  <Table.Td>{edit.kind.replace('_', ' ')}</Table.Td>
                  <Table.Td className="mono">{edit.schemaPath}</Table.Td>
                  <Table.Td>{edit.formatId}</Table.Td>
                </Table.Tr>
              ))
            )}
          </Table.Tbody>
        </Table>
      </Paper>

      <ValidationPanel change={change} />
      <ReviewsPanel change={change} />
      {change.applyResult ? <ApplyResultPanel result={change.applyResult} /> : null}
    </Stack>
  );
}

function ValidationPanel({ change }: { change: ChangeRecord }) {
  const validation = change.validation;
  return (
    <Paper withBorder p="md">
      <Group justify="space-between">
        <Title order={4}>Validation snapshot</Title>
        {validation ? (
          <Badge color={validation.valid ? 'green' : 'red'} variant="light">
            {validation.valid ? 'passing' : 'blocked'}
          </Badge>
        ) : (
          <Badge color="gray" variant="light">
            not run
          </Badge>
        )}
      </Group>
      {!validation ? (
        <Text size="sm" c="dimmed" mt="sm">
          Validation has not been recorded for the current edits.
        </Text>
      ) : (
        <Stack mt="sm" gap="xs">
          <Text size="sm">
            Base <Code>{validation.resolvedBaseCommit}</Code> · validator{' '}
            <Code>{validation.validatorVersion}</Code>
          </Text>
          <Text size="xs" c="dimmed" className="mono">
            {validation.editDigest}
          </Text>
          {validation.issues.map((issue, index) => (
            <Alert key={`${issue.code}-${index}`} color="red" title={issue.code}>
              {issue.message}
              {issue.schemaName ? ` · ${issue.schemaName}` : ''}
              {issue.declarationName ? ` · ${issue.declarationName}` : ''}
            </Alert>
          ))}
        </Stack>
      )}
    </Paper>
  );
}

function ReviewsPanel({ change }: { change: ChangeRecord }) {
  return (
    <Paper withBorder p="md">
      <Title order={4}>Reviews</Title>
      {change.reviews.length === 0 ? (
        <Text size="sm" c="dimmed" mt="sm">
          No human or maintainer review has been recorded.
        </Text>
      ) : (
        <Stack mt="sm" gap="xs">
          {change.reviews.map((review) => (
            <Group key={`${review.reviewer.identity}-${review.createTimeUnixMs}`} justify="space-between">
              <div>
                <Text fw={600}>{review.reviewer.displayName || review.reviewer.identity}</Text>
                <Text size="sm" c="dimmed">
                  {review.reason || 'No reason supplied'}
                </Text>
              </div>
              <Badge color={review.decision === 'approved' ? 'green' : 'red'}>
                {review.decision}
              </Badge>
            </Group>
          ))}
        </Stack>
      )}
    </Paper>
  );
}

function ApplyResultPanel({ result }: { result: ChangeApplyResult }) {
  return (
    <Paper withBorder p="md">
      <Group gap="xs">
        <CheckCircle2 size={20} color="green" />
        <Title order={4}>Immutable apply receipt</Title>
      </Group>
      <SimpleGrid cols={{ base: 1, md: 3 }} mt="md">
        <div>
          <Text size="xs" c="dimmed">Commit</Text>
          <Code>{result.commitId}</Code>
        </div>
        <div>
          <Text size="xs" c="dimmed">JJ change</Text>
          <Code>{result.changeId}</Code>
        </div>
        <div>
          <Text size="xs" c="dimmed">Operation</Text>
          <Code>{result.operationId}</Code>
        </div>
      </SimpleGrid>
      <Group mt="md" gap="xs">
        <GitCommit size={16} />
        <Text size="sm">
          {result.conflictedDeclarations.length === 0
            ? 'Applied without declaration conflicts.'
            : `Conflicts: ${result.conflictedDeclarations.join(', ')}`}
        </Text>
      </Group>
      {result.artifactDigest ? (
        <Text size="xs" c="dimmed" className="mono" mt="xs">
          {result.artifactDigest}
        </Text>
      ) : null}
    </Paper>
  );
}
