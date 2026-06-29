import {
  AppShell as MantineAppShell,
  Badge,
  Burger,
  Group,
  NavLink,
  ScrollArea,
  Text,
  TextInput,
  Title,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import {
  Boxes,
  Code2,
  GitCompare,
  History,
  LayoutDashboard,
  Search,
  Settings,
} from 'lucide-react';
import { Link, Navigate, Route, Routes, useLocation } from 'react-router-dom';

import { AdminPage } from './pages/AdminPage';
import { ComparePage } from './pages/ComparePage';
import { HistoryPage } from './pages/HistoryPage';
import { ProjectsPage } from './pages/ProjectsPage';
import { RepoDashboardPage } from './pages/RepoDashboardPage';
import { SchemaDetailPage } from './pages/SchemaDetailPage';

const project = 'acme';
const repo = 'commerce';

function ShellNav() {
  const location = useLocation();
  const repoBase = `/projects/${project}/repos/${repo}`;

  return (
    <ScrollArea h="100%">
      <div className="navSectionLabel">Workspace</div>
      <NavLink
        component={Link}
        to="/projects"
        label="Projects"
        leftSection={<Boxes size={16} />}
        active={location.pathname === '/projects'}
      />
      <div className="navSectionLabel">acme / commerce</div>
      <NavLink
        component={Link}
        to={repoBase}
        label="Dashboard"
        leftSection={<LayoutDashboard size={16} />}
        active={location.pathname === repoBase}
      />
      <NavLink
        component={Link}
        to={`${repoBase}/schemas/order.proto?ref=main`}
        label="Schema Detail"
        leftSection={<Code2 size={16} />}
        active={location.pathname.includes('/schemas/')}
      />
      <NavLink
        component={Link}
        to={`${repoBase}/compare?base=tag:release-2026-06-05&head=main`}
        label="Compare"
        leftSection={<GitCompare size={16} />}
        active={location.pathname.endsWith('/compare')}
      />
      <NavLink
        component={Link}
        to={`${repoBase}/history?ref=main`}
        label="History"
        leftSection={<History size={16} />}
        active={location.pathname.endsWith('/history')}
      />
      <div className="navSectionLabel">System</div>
      <NavLink
        component={Link}
        to="/admin"
        label="Admin"
        leftSection={<Settings size={16} />}
        active={location.pathname === '/admin'}
      />
    </ScrollArea>
  );
}

export function App() {
  const [opened, { toggle }] = useDisclosure();

  return (
    <MantineAppShell
      header={{ height: 56 }}
      navbar={{
        width: 260,
        breakpoint: 'sm',
        collapsed: { mobile: !opened },
      }}
      padding="md"
    >
      <MantineAppShell.Header>
        <Group h="100%" px="md" justify="space-between">
          <Group>
            <Burger opened={opened} onClick={toggle} hiddenFrom="sm" size="sm" />
            <Title order={3}>SchemaHub</Title>
            <Badge variant="light" color="blue">
              mock console
            </Badge>
          </Group>
          <TextInput
            visibleFrom="sm"
            leftSection={<Search size={16} />}
            placeholder="Search schemas, declarations, refs, operations"
            w={420}
          />
          <Group gap="xs">
            <Badge color="green" variant="light">
              redb
            </Badge>
            <Text size="sm" c="dimmed">
              Anonymous
            </Text>
          </Group>
        </Group>
      </MantineAppShell.Header>

      <MantineAppShell.Navbar p="xs">
        <ShellNav />
      </MantineAppShell.Navbar>

      <MantineAppShell.Main className="shellMain">
        <Routes>
          <Route path="/" element={<Navigate to={`/projects/${project}/repos/${repo}`} replace />} />
          <Route path="/projects" element={<ProjectsPage />} />
          <Route path="/projects/:project/repos/:repo" element={<RepoDashboardPage />} />
          <Route path="/projects/:project/repos/:repo/schemas/*" element={<SchemaDetailPage />} />
          <Route path="/projects/:project/repos/:repo/compare" element={<ComparePage />} />
          <Route path="/projects/:project/repos/:repo/history" element={<HistoryPage />} />
          <Route path="/admin" element={<AdminPage />} />
        </Routes>
      </MantineAppShell.Main>
    </MantineAppShell>
  );
}

