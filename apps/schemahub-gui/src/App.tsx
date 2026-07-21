import {
  AppShell as MantineAppShell,
  Badge,
  Burger,
  Group,
  NavLink,
  ScrollArea,
  TextInput,
  Title,
} from '@mantine/core';
import { useDisclosure } from '@mantine/hooks';
import {
  Boxes,
  Code2,
  GitCompare,
  GitMerge,
  GitPullRequest,
  History,
  LayoutDashboard,
  Search,
  Settings,
} from 'lucide-react';
import { FormEvent, useState } from 'react';
import {
  Link,
  Navigate,
  Route,
  Routes,
  matchPath,
  useLocation,
  useNavigate,
} from 'react-router-dom';

import { schemaHubMode } from './api';
import { useRepository, useServerConfig } from './api/queries';
import { IdentityMenu } from './components/IdentityMenu';
import { AdminPage } from './pages/AdminPage';
import { ChangeDetailPage } from './pages/ChangeDetailPage';
import { ChangesPage } from './pages/ChangesPage';
import { ComparePage } from './pages/ComparePage';
import { ConflictsPage } from './pages/ConflictsPage';
import { HistoryPage } from './pages/HistoryPage';
import { ProjectsPage } from './pages/ProjectsPage';
import { ProjectPage } from './pages/ProjectPage';
import { RepoDashboardPage } from './pages/RepoDashboardPage';
import { SchemaDetailPage } from './pages/SchemaDetailPage';
import { SearchPage } from './pages/SearchPage';

function ShellNav() {
  const location = useLocation();
  const repoMatch =
    matchPath('/projects/:project/repos/:repo/*', location.pathname) ??
    matchPath('/projects/:project/repos/:repo', location.pathname);
  const project = repoMatch?.params.project;
  const repo = repoMatch?.params.repo;
  const repoBase = project && repo ? `/projects/${project}/repos/${repo}` : undefined;
  const { data: repository } = useRepository(project || '', repo || '');
  const defaultRef = repository?.defaultBranch;

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
      {repoBase && project && repo ? (
        <>
          <div className="navSectionLabel">
            {project} / {repo}
          </div>
          <NavLink
            component={Link}
            to={`/projects/${project}`}
            label="Repositories"
            leftSection={<Boxes size={16} />}
          />
          <NavLink
            component={Link}
            to={repoBase}
            label="Dashboard"
            leftSection={<LayoutDashboard size={16} />}
            active={location.pathname === repoBase}
          />
          <NavLink
            component={Link}
            to={`${repoBase}/changes`}
            label="Changes"
            leftSection={<GitPullRequest size={16} />}
            active={location.pathname.includes(`${repoBase}/changes`)}
          />
          <NavLink
            component={Link}
            to={
              defaultRef
                ? `${repoBase}/conflicts?bookmark=${encodeURIComponent(defaultRef)}`
                : `${repoBase}/conflicts`
            }
            label="Conflicts"
            leftSection={<GitMerge size={16} />}
            active={location.pathname.endsWith('/conflicts')}
          />
          <NavLink
            component={Link}
            to={`${repoBase}/search`}
            label="Search"
            leftSection={<Search size={16} />}
            active={location.pathname.endsWith('/search')}
          />
          {location.pathname.includes('/schemas/') ? (
            <NavLink
              component={Link}
              to={`${location.pathname}${location.search}`}
              label="Schema Detail"
              leftSection={<Code2 size={16} />}
              active
            />
          ) : null}
          <NavLink
            component={Link}
            to={
              defaultRef
                ? `${repoBase}/compare?base=${encodeURIComponent(defaultRef)}&head=${encodeURIComponent(defaultRef)}`
                : `${repoBase}/compare`
            }
            label="Compare"
            leftSection={<GitCompare size={16} />}
            active={location.pathname.endsWith('/compare')}
          />
          <NavLink
            component={Link}
            to={defaultRef ? `${repoBase}/history?ref=${encodeURIComponent(defaultRef)}` : `${repoBase}/history`}
            label="History"
            leftSection={<History size={16} />}
            active={location.pathname.endsWith('/history')}
          />
        </>
      ) : null}
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

function GlobalSearch() {
  const location = useLocation();
  const navigate = useNavigate();
  const [query, setQuery] = useState('');
  const match =
    matchPath('/projects/:project/repos/:repo/*', location.pathname) ??
    matchPath('/projects/:project/repos/:repo', location.pathname);
  const project = match?.params.project;
  const repo = match?.params.repo;

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const normalized = query.trim();
    if (!project || !repo || !normalized) return;
    navigate(
      `/projects/${encodeURIComponent(project)}/repos/${encodeURIComponent(repo)}/search?q=${encodeURIComponent(normalized)}`,
    );
  }

  return (
    <form onSubmit={submit}>
      <TextInput
        visibleFrom="sm"
        leftSection={<Search size={16} />}
        placeholder={
          project && repo
            ? 'Search schemas, declarations, revisions, changes'
            : 'Open a repository to search'
        }
        aria-label="Repository search"
        value={query}
        onChange={(event) => setQuery(event.currentTarget.value)}
        disabled={!project || !repo}
        w={420}
      />
    </form>
  );
}

export function App() {
  const [opened, { toggle }] = useDisclosure();
  const { data: serverConfig } = useServerConfig();

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
              {schemaHubMode === 'live' ? 'live API' : 'demo data'}
            </Badge>
          </Group>
          <GlobalSearch />
          <Group gap="xs">
            <Badge color="green" variant="light">
              {serverConfig?.storageBackend || 'server'}
            </Badge>
            <IdentityMenu />
          </Group>
        </Group>
      </MantineAppShell.Header>

      <MantineAppShell.Navbar p="xs">
        <ShellNav />
      </MantineAppShell.Navbar>

      <MantineAppShell.Main className="shellMain">
        <Routes>
          <Route path="/" element={<Navigate to="/projects" replace />} />
          <Route path="/projects" element={<ProjectsPage />} />
          <Route path="/projects/:project" element={<ProjectPage />} />
          <Route path="/projects/:project/repos/:repo" element={<RepoDashboardPage />} />
          <Route path="/projects/:project/repos/:repo/changes" element={<ChangesPage />} />
          <Route
            path="/projects/:project/repos/:repo/changes/:changeId"
            element={<ChangeDetailPage />}
          />
          <Route path="/projects/:project/repos/:repo/conflicts" element={<ConflictsPage />} />
          <Route path="/projects/:project/repos/:repo/schemas/*" element={<SchemaDetailPage />} />
          <Route path="/projects/:project/repos/:repo/compare" element={<ComparePage />} />
          <Route path="/projects/:project/repos/:repo/history" element={<HistoryPage />} />
          <Route path="/projects/:project/repos/:repo/search" element={<SearchPage />} />
          <Route path="/admin" element={<AdminPage />} />
        </Routes>
      </MantineAppShell.Main>
    </MantineAppShell>
  );
}
