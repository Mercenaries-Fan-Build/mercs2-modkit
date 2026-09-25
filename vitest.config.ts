import { defineConfig } from "vitest/config";

// Unit tests for the frontend stores. They run in Node with no DOM: each test stubs the Tauri
// bridge it reaches, so nothing talks to a backend.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
