import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    proxy: {
      "/healthz": "http://127.0.0.1:8073",
      "/api": { target: "http://127.0.0.1:8073", ws: true },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
