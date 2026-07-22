/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_SCHEMAHUB_API_BASE?: string;
  readonly VITE_SCHEMAHUB_TOKEN?: string;
  readonly VITE_SCHEMAHUB_USE_MOCKS?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
