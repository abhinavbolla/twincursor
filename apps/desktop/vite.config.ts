import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri serves `dist` from inside the app and points at this dev server in `tauri dev`.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    target: "safari16",
    rollupOptions: {
      input: { main: "index.html", probe: "probe.html" },
    },
  },
  test: { include: ["src/**/*.test.ts"] },
});
