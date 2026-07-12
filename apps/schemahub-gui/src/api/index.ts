import { HttpSchemaHubClient } from './httpClient';
import { MockSchemaHubClient } from './mockClient';
import type { SchemaHubClient } from './client';

const apiBase = import.meta.env.VITE_SCHEMAHUB_API_BASE as string | undefined;

export const schemaHubClient: SchemaHubClient = apiBase
  ? new HttpSchemaHubClient(apiBase.replace(/\/$/, ''))
  : new MockSchemaHubClient();
