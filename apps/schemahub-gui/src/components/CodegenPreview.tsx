import { useMemo, useState } from 'react';
import {
  Alert,
  Button,
  Group,
  Paper,
  SegmentedControl,
  Stack,
  Switch,
  Text,
  Tooltip,
} from '@mantine/core';
import { useMutation } from '@tanstack/react-query';
import { Code2, Download } from 'lucide-react';

import { schemaHubClient } from '../api';
import type { ArtifactDownload, ArtifactKind, SchemaFormat } from '../api/types';
import { CodeViewer } from './CodeViewer';

type CodegenPreviewProps = {
  project: string;
  repo: string;
  schemaPath: string;
  format: SchemaFormat;
  refName: string;
};

function languageFor(format: SchemaFormat, language: string) {
  if (format === 'openapi') return 'yaml';
  if (language === 'typescript') return 'typescript';
  return 'rust';
}

export function CodegenPreviewPanel({
  project,
  repo,
  schemaPath,
  format,
  refName,
}: CodegenPreviewProps) {
  const [language, setLanguage] = useState<'rust' | 'typescript'>('rust');
  const [rustPluggableBuffer, setRustPluggableBuffer] = useState(false);
  const [lastArtifact, setLastArtifact] = useState<ArtifactDownload>();
  const pluggableEnabled = format === 'flatbuffers' && language === 'rust';

  const preview = useMutation({
    mutationFn: () =>
      schemaHubClient.previewCodegen({
        project,
        repo,
        schemaPath,
        ref: refName,
        language,
        rustPluggableBuffer: pluggableEnabled && rustPluggableBuffer,
      }),
  });
  const artifact = useMutation({
    mutationFn: (kind: ArtifactKind) =>
      schemaHubClient.downloadArtifact({
        project,
        repo,
        schemaPath,
        ref: refName,
        kind,
        language: kind === 'generated-code' ? language : undefined,
        rustPluggableBuffer: pluggableEnabled && rustPluggableBuffer,
      }),
    onSuccess: (download, kind) => {
      setLastArtifact(download);
      saveBlob(download.content, artifactFilename(schemaPath, kind, language));
    },
  });

  const viewerLanguage = useMemo(
    () => languageFor(format, language),
    [format, language],
  );

  return (
    <Stack gap="md">
      <Paper withBorder p="md" radius="sm">
        <Group justify="space-between" align="center">
          <Group>
            <SegmentedControl
              value={language}
              onChange={(value) => setLanguage(value as 'rust' | 'typescript')}
              data={[
                { value: 'rust', label: 'Rust' },
                { value: 'typescript', label: 'TypeScript' },
              ]}
            />
            <Tooltip
              label={
                pluggableEnabled
                  ? 'Generate FlatBuffers readers over FlatBufferRead.'
                  : 'Only available for FlatBuffers Rust output.'
              }
            >
              <Switch
                checked={rustPluggableBuffer}
                disabled={!pluggableEnabled}
                onChange={(event) => setRustPluggableBuffer(event.currentTarget.checked)}
                label="Pluggable buffer"
              />
            </Tooltip>
          </Group>
          <Group>
            <Button
              leftSection={<Code2 size={16} />}
              loading={preview.isPending}
              onClick={() => preview.mutate()}
            >
              Preview
            </Button>
            <Button
              variant="light"
              leftSection={<Download size={16} />}
              loading={artifact.isPending && artifact.variables === 'source'}
              onClick={() => artifact.mutate('source')}
            >
              Source
            </Button>
            <Button
              variant="light"
              leftSection={<Download size={16} />}
              loading={artifact.isPending && artifact.variables === 'descriptors'}
              onClick={() => artifact.mutate('descriptors')}
            >
              Descriptor
            </Button>
            <Button
              variant="light"
              leftSection={<Download size={16} />}
              loading={artifact.isPending && artifact.variables === 'generated-code'}
              disabled={format === 'openapi'}
              onClick={() => artifact.mutate('generated-code')}
            >
              Generated
            </Button>
          </Group>
        </Group>
      </Paper>

      {format === 'openapi' ? (
        <Alert color="yellow" title="Codegen unsupported">
          OpenAPI codegen is not implemented in SchemaHub v1. Descriptor preview remains available.
        </Alert>
      ) : null}

      {artifact.error ? (
        <Alert color="red" title="Artifact download failed">
          {artifact.error.message}
        </Alert>
      ) : null}

      {lastArtifact ? (
        <Alert color="green" title="Immutable artifact downloaded">
          Resolved <span className="mono">{lastArtifact.revision.resolvedFrom}</span> to commit{' '}
          <span className="mono">{lastArtifact.revision.commitId}</span>. Artifact{' '}
          <span className="mono">{lastArtifact.artifactDigest}</span>; closure{' '}
          <span className="mono">{lastArtifact.closureDigest}</span>.
        </Alert>
      ) : null}

      {preview.data ? (
        <Stack gap="xs">
          <Text size="sm" c="dimmed">
            Resolved commit <span className="mono">{preview.data.atCommit}</span>
          </Text>
          <CodeViewer value={preview.data.content} language={viewerLanguage} height={460} />
        </Stack>
      ) : (
        <Paper withBorder p="xl" radius="sm">
          <Text c="dimmed">Run preview to render generated source for this schema and ref.</Text>
        </Paper>
      )}
    </Stack>
  );
}

function artifactFilename(
  schemaPath: string,
  kind: ArtifactKind,
  language: 'rust' | 'typescript',
) {
  const sourceName = schemaPath.split('/').pop() || 'schema';
  if (kind === 'source') return sourceName;
  const stem = sourceName.replace(/\.[^.]+$/, '');
  if (kind === 'descriptors') return `${stem}.schemahub.desc`;
  return `${stem}.schemahub.${language === 'typescript' ? 'ts' : 'rs'}`;
}

function saveBlob(content: Blob, filename: string) {
  const url = URL.createObjectURL(content);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
