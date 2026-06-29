import { Badge } from '@mantine/core';
import type { CompatibilityState, SchemaFormat } from '../api/types';

export function FormatBadge({ format }: { format: SchemaFormat }) {
  const color = format === 'protobuf' ? 'blue' : format === 'flatbuffers' ? 'teal' : 'grape';
  return (
    <Badge color={color} variant="light" radius="sm">
      {format}
    </Badge>
  );
}

export function CompatibilityBadge({ state }: { state: CompatibilityState }) {
  const color =
    state === 'compatible'
      ? 'green'
      : state === 'warning'
        ? 'yellow'
        : state === 'breaking'
          ? 'red'
          : 'gray';
  return (
    <Badge color={color} variant="light" radius="sm">
      {state}
    </Badge>
  );
}

export function RefBadge({ refName }: { refName: string }) {
  const color = refName.startsWith('tag:') ? 'violet' : refName.startsWith('@') ? 'gray' : 'blue';
  return (
    <Badge color={color} variant="light" radius="sm" className="mono">
      {refName}
    </Badge>
  );
}

