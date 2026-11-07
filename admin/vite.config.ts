import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { mkdir, copyFile } from "node:fs/promises";
export default defineConfig({
  base: "./",
  // public/catalog contains runtime uploads in local development. Never bundle it.
  publicDir: false,
  plugins: [
    react(),
    {
      name: "copy-server-entrypoints",
      async closeBundle() {
        await mkdir(new URL("./dist/api/", import.meta.url), {
          recursive: true,
        });
        for (const path of ["api/index.php", "api/mcp.php", ".htaccess"]) {
          await copyFile(
            new URL(`./public/${path}`, import.meta.url),
            new URL(`./dist/${path}`, import.meta.url),
          );
        }
      },
    },
  ],
  server: {
    proxy: {
      "/api": "http://127.0.0.1:8787",
      "/catalog": "http://127.0.0.1:8787",
    },
  },
});
