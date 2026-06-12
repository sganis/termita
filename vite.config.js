// vite.config.js
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Frontend builds to web/dist; server.js serves that folder.
// During `bun run dev`, proxy /ws to the Bun server on :3000.
export default defineConfig({
  plugins: [svelte()],
  build: {
    outDir: "web/dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/ws": { target: "ws://localhost:3000", ws: true },
    },
  },
});
