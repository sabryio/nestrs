import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

/** Where the Rust `blog-api` listens (`BLOG_BIND_ADDR`, default 127.0.0.1:3000). */
const api = process.env.API_ORIGIN ?? "http://127.0.0.1:3000";

export default defineConfig({
  plugins: [react()],
  server: {
    // Same-origin in dev: the browser talks to Vite, Vite forwards `/api` to Rust.
    // Server-Sent Events pass through the proxy unbuffered.
    proxy: { "/api": { target: api, changeOrigin: true } },
  },
  test: { include: ["e2e/**/*.test.ts", "scripts/**/*.test.ts"], testTimeout: 30_000, hookTimeout: 120_000 },
});
