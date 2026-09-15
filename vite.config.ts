/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  build: {
    // WKWebView on macOS 11+ and WebView2 on Windows — both evergreen enough for ES2021.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari15",
    // Vite 8 bundles with rolldown; `true` uses its built-in minifier (no esbuild dependency).
    minify: !process.env.TAURI_ENV_DEBUG,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    // `scripts/check-bundle-size.mjs` gates the result (ARCHITECTURE §12, §13).
    chunkSizeWarningLimit: 600,
  },

  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    css: false,
    restoreMocks: true,
    env: {
      // every test runs against the mock adapter — there is no Rust in vitest
      VITE_SEEPDF_MOCK: "1",
    },
  },
}));
