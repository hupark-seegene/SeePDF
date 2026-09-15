/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `VITE_SEEPDF_MOCK=1` runs the whole frontend against `src/ipc/mock.ts` (IPC_CONTRACT §13). */
  readonly VITE_SEEPDF_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
