/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// `process` is now typed via @types/node (added for the Node-based test
// helpers), so this no longer needs a `@ts-expect-error` suppression.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Let Rust compile errors stay on screen during `tauri dev`.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // The webview is a fixed, current WebKit — no legacy transpilation needed.
    target: "safari15",
    sourcemap: false,
  },
  test: {
    // Pure-logic tests only so far (lib/format.ts, the shared IPC-shape
    // contract) — none of it touches the DOM, so the default Node
    // environment is enough and there's no jsdom dependency to add.
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
