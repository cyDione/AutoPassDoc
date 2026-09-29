import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and does not need Vite to clear the terminal.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // The About page imports ../CHANGELOG.md.
  server: { port: 5173, strictPort: true, fs: { allow: [".."] } },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2022", sourcemap: false },
});
