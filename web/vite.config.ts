import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Backend for the dev proxy; override to run next to another stack on :8073.
const apiTarget = process.env.SUPERHET_DEV_API ?? "http://127.0.0.1:8073";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/healthz": apiTarget,
      "/api": { target: apiTarget, ws: true },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
