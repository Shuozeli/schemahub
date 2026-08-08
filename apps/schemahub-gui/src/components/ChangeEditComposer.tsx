import {
  ActionIcon,
  Button,
  Group,
  Paper,
  Select,
  SimpleGrid,
  Stack,
  Text,
  Textarea,
  TextInput,
} from '@mantine/core';
import { FilePlus2, Trash2 } from 'lucide-react';

import type { ChangeEditInput, SchemaFormat } from '../api/types';

const formatOptions: { label: string; value: SchemaFormat }[] = [
  { label: 'Protobuf', value: 'protobuf' },
  { label: 'FlatBuffers', value: 'flatbuffers' },
  { label: 'OpenAPI 3.1', value: 'openapi' },
];

const kindOptions = [
  { label: 'Create or replace source', value: 'replace_source' },
  { label: 'Delete schema', value: 'delete_schema' },
];

export function ChangeEditComposer({
  value,
  onChange,
  disabled = false,
}: {
  value: ChangeEditInput[];
  onChange: (next: ChangeEditInput[]) => void;
  disabled?: boolean;
}) {
  function replace(index: number, edit: ChangeEditInput) {
    onChange(value.map((candidate, candidateIndex) => (candidateIndex === index ? edit : candidate)));
  }

  return (
    <Stack gap="sm">
      {value.length === 0 ? (
        <Paper withBorder p="md">
          <Text size="sm" c="dimmed">
            No executable edits. The proposal will preserve intent as a note-only draft.
          </Text>
        </Paper>
      ) : null}

      {value.map((edit, index) => (
        <Paper withBorder p="md" key={`${index}-${edit.kind}`}>
          <Stack gap="sm">
            <Group justify="space-between">
              <Text fw={600}>Executable edit {index + 1}</Text>
              <ActionIcon
                variant="subtle"
                color="red"
                aria-label={`Remove executable edit ${index + 1}`}
                onClick={() => onChange(value.filter((_, candidateIndex) => candidateIndex !== index))}
                disabled={disabled}
              >
                <Trash2 size={16} />
              </ActionIcon>
            </Group>
            <SimpleGrid cols={{ base: 1, sm: 2 }}>
              <Select
                label="Edit kind"
                data={kindOptions}
                value={edit.kind}
                onChange={(nextKind) => {
                  if (nextKind === 'replace_source') {
                    replace(index, {
                      kind: 'replace_source',
                      schemaPath: edit.schemaPath,
                      formatId: edit.formatId,
                      source: '',
                    });
                  } else if (nextKind === 'delete_schema') {
                    replace(index, {
                      kind: 'delete_schema',
                      schemaPath: edit.schemaPath,
                      formatId: edit.formatId,
                    });
                  }
                }}
                allowDeselect={false}
                disabled={disabled}
              />
              <Select
                label="Schema format"
                data={formatOptions}
                value={edit.formatId}
                onChange={(formatId) => {
                  if (isSchemaFormat(formatId)) replace(index, { ...edit, formatId });
                }}
                allowDeselect={false}
                disabled={disabled}
              />
            </SimpleGrid>
            <TextInput
              label="Schema path"
              description={`Use a ${extensionHint(edit.formatId)} path relative to this repository`}
              placeholder={pathPlaceholder(edit.formatId)}
              value={edit.schemaPath}
              onChange={(event) => replace(index, { ...edit, schemaPath: event.currentTarget.value })}
              error={
                edit.schemaPath.trim() &&
                !schemaPathMatchesFormat(edit.schemaPath.trim(), edit.formatId)
                  ? `Path must use ${extensionHint(edit.formatId)} for ${edit.formatId}.`
                  : undefined
              }
              disabled={disabled}
              required
            />
            {edit.kind === 'replace_source' ? (
              <Textarea
                label="Complete schema source"
                description="Validation parses this source and records compatibility findings before review."
                placeholder={sourcePlaceholder(edit.formatId)}
                value={edit.source}
                onChange={(event) => replace(index, { ...edit, source: event.currentTarget.value })}
                autosize
                minRows={10}
                maxRows={24}
                classNames={{ input: 'mono' }}
                disabled={disabled}
                required
              />
            ) : (
              <Text size="sm" c="dimmed">
                Deletion is executable but cannot be applied while live dependents still reference
                this schema.
              </Text>
            )}
          </Stack>
        </Paper>
      ))}

      <Button
        variant="light"
        leftSection={<FilePlus2 size={16} />}
        onClick={() => onChange([...value, emptyChangeEdit()])}
        disabled={disabled || value.length >= 100}
        w="fit-content"
      >
        Add executable edit
      </Button>
    </Stack>
  );
}

export function emptyChangeEdit(formatId: SchemaFormat = 'protobuf'): ChangeEditInput {
  return {
    kind: 'replace_source',
    schemaPath: '',
    formatId,
    source: '',
  };
}

export function changeEditsAreComplete(edits: ChangeEditInput[]) {
  return edits.every(
    (edit) =>
      edit.schemaPath.trim().length > 0 &&
      schemaPathMatchesFormat(edit.schemaPath.trim(), edit.formatId) &&
      (edit.kind === 'delete_schema' || edit.source.trim().length > 0),
  );
}

export function prepareChangeEdits(edits: ChangeEditInput[]): ChangeEditInput[] {
  return edits.map((edit) => ({
    ...edit,
    schemaPath: edit.schemaPath.trim(),
  }));
}

function isSchemaFormat(value: string | null): value is SchemaFormat {
  return value === 'protobuf' || value === 'flatbuffers' || value === 'openapi';
}

function schemaPathMatchesFormat(schemaPath: string, format: SchemaFormat) {
  if (format === 'protobuf') return schemaPath.endsWith('.proto');
  if (format === 'flatbuffers') return schemaPath.endsWith('.fbs');
  return (
    schemaPath.endsWith('.yaml') ||
    schemaPath.endsWith('.yml') ||
    schemaPath.endsWith('.json')
  );
}

function extensionHint(format: SchemaFormat) {
  if (format === 'protobuf') return '.proto';
  if (format === 'flatbuffers') return '.fbs';
  return '.yaml, .yml, or .json';
}

function pathPlaceholder(format: SchemaFormat) {
  if (format === 'protobuf') return 'commerce/v1/order.proto';
  if (format === 'flatbuffers') return 'telemetry/v1/event.fbs';
  return 'payments/v1/openapi.yaml';
}

function sourcePlaceholder(format: SchemaFormat) {
  if (format === 'protobuf') {
    return 'syntax = "proto3";\\n\\npackage commerce.v1;\\n\\nmessage Order {}';
  }
  if (format === 'flatbuffers') {
    return 'namespace telemetry.v1;\\n\\ntable Event {}\\n\\nroot_type Event;';
  }
  return 'openapi: 3.1.0\\ninfo:\\n  title: Payments\\n  version: 1.0.0\\npaths: {}';
}
