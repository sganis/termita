// web/vite.config.js
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Frontend builds to ./dist (i.e. web/dist); the Rust server embeds that folder
// at compile time. During `bun run dev`, proxy /ws to the backend on :3000.
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/ws": { target: "ws://localhost:3000", ws: true },
    },
  },
});
