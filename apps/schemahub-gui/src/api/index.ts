import { HttpSchemaHubClient } from './httpClient';
import { MockSchemaHubClient } from './mockClient';
import type { SchemaHubClient } from './client';

const apiBase = import.meta.env.VITE_SCHEMAHUB_API_BASE as string | undefined;
const configuredToken = import.meta.env.VITE_SCHEMAHUB_TOKEN as string | undefined;
const useMocks = import.meta.env.VITE_SCHEMAHUB_USE_MOCKS === 'true';

function browserToken() {
  return window.localStorage.getItem('schemahub.token') || configuredToken;
}

export const schemaHubMode = useMocks ? 'demo' : 'live';

export const schemaHubClient: SchemaHubClient = useMocks
  ? new MockSchemaHubClient()
  : new HttpSchemaHubClient((apiBase || '').replace(/\/$/, ''), browserToken);
