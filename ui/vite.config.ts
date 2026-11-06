import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const page = (file: string) => fileURLToPath(new URL(file, import.meta.url));

// Static assets only; the app is served by Tauri from ui/dist (no dev server in production).
// Two pages: the main window and the optional desktop widget (Phase 3.7).
export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false,
    rolldownOptions: { input: { main: page("index.html"), widget: page("widget.html") } },
  },
  server: { port: 5173, strictPort: true },
});
