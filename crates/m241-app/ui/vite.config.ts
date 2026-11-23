import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The Tauri window loads `dist/` through `tauri.conf.json` (`frontendDist`).
// `clearScreen` and the fixed dev port are the values a `tauri dev` run expects;
// P0 builds the assets first and starts the binary with `cargo run`, so the dev
// server is not part of the normal loop yet.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "es2022",
  },
});
