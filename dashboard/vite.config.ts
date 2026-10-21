import { defineConfig } from "vitest/config";
export default defineConfig({
  server: {
    proxy: { "/api": { target: "http://127.0.0.1:8080", changeOrigin: false } },
  },
  test: { environment: "jsdom", restoreMocks: true },
  build: { sourcemap: false },
});
