import { defineConfig } from "vitest/config";

// Pure-function tests only (no DOM): kept apart from vite.config.ts so the Tauri dev
// server options stay untouched.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
