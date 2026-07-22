import { Badge, Button, Group, PasswordInput, Popover, Stack, Text } from '@mantine/core';
import { useQueryClient } from '@tanstack/react-query';
import { Bot, KeyRound, UserRound } from 'lucide-react';
import { useState } from 'react';

import { useSession } from '../api/queries';

export function IdentityMenu() {
  const queryClient = useQueryClient();
  const { data: session, isLoading } = useSession();
  const [token, setToken] = useState(() => window.localStorage.getItem('schemahub.token') || '');
  const [opened, setOpened] = useState(false);

  const saveToken = async () => {
    const normalized = token.trim();
    if (normalized) {
      window.localStorage.setItem('schemahub.token', normalized);
    } else {
      window.localStorage.removeItem('schemahub.token');
    }
    setOpened(false);
    await queryClient.invalidateQueries();
  };

  const clearToken = async () => {
    setToken('');
    window.localStorage.removeItem('schemahub.token');
    setOpened(false);
    await queryClient.invalidateQueries();
  };

  const label = isLoading ? 'Resolving identity…' : session?.display || session?.id || 'Anonymous';
  const PrincipalIcon = session?.kind === 'agent' ? Bot : UserRound;

  return (
    <Popover opened={opened} onChange={setOpened} width={340} position="bottom-end" withArrow>
      <Popover.Target>
        <Button
          variant="subtle"
          color="gray"
          leftSection={<PrincipalIcon size={16} />}
          onClick={() => setOpened((value) => !value)}
        >
          {label}
        </Button>
      </Popover.Target>
      <Popover.Dropdown>
        <Stack gap="sm">
          <div>
            <Group gap="xs">
              <Text fw={700}>{label}</Text>
              <Badge variant="light">{session?.kind || 'anonymous'}</Badge>
            </Group>
            {session?.delegatedBy ? (
              <Text size="xs" c="dimmed">
                Delegated by {session.delegatedBy}
              </Text>
            ) : null}
          </div>
          <PasswordInput
            label="Bearer token"
            description="Stored only in this browser and sent to the SchemaHub BFF."
            leftSection={<KeyRound size={15} />}
            value={token}
            onChange={(event) => setToken(event.currentTarget.value)}
            placeholder="Paste a human, agent, or service token"
          />
          <Group justify="flex-end">
            <Button variant="default" onClick={clearToken}>
              Clear
            </Button>
            <Button onClick={saveToken}>Use token</Button>
          </Group>
        </Stack>
      </Popover.Dropdown>
    </Popover>
  );
}
