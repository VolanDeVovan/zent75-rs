import { defineConfig } from "vitest/config";

// Static app: relative base so dist/ can be served from any path or opened from a sub-folder.
export default defineConfig({
  base: "./",
  build: { target: "es2022", sourcemap: true },
  test: { include: ["tests/**/*.test.ts"], environment: "node" },
});
