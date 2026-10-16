import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },

  // Code-splitting manual chunks: isolates the heavy 3D graph libraries (three,
  // react-force-graph-3d, d3-*) and React vendor into dedicated chunks so app
  // startup does not parse them until Stats is first visited.
  build: {
    target: "chrome110",
    rollupOptions: {
      output: {
        manualChunks(id) {
          const normalized = id.replace(/\\/g, "/");
          if (normalized.includes("/node_modules/")) {
            if (
              normalized.includes("/three/") ||
              normalized.includes("/three-") ||
              normalized.includes("/react-force-graph-3d") ||
              normalized.includes("/d3-")
            ) {
              return "graph3d";
            }
            if (
              normalized.includes("/react/") ||
              normalized.includes("/react-dom/")
            ) {
              return "vendor";
            }
          }
        },
      },
    },
  },
}));
