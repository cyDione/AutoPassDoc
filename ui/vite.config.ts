import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and does not need Vite to clear the terminal.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2022", sourcemap: false },
});
