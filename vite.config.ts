import { defineConfig } from "vite";
import { readFileSync } from "fs";
import vue from "@vitejs/plugin-vue";

const host = process.env.TAURI_DEV_HOST;
const packageJson = JSON.parse(readFileSync('./package.json', 'utf-8'))

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [
    vue()
  ],

  // Relative asset URLs ("./assets/…"), not Vite's default absolute ones
  // ("/assets/…"). Windows' WebView2 resolves the absolute form fine against
  // Tauri's custom protocol, which is why this went unnoticed; the Linux
  // webkit side is the one that's historically fussy about it, and it fails
  // in exactly the shape seen on the Deck -- a white page with NOTHING on
  // stderr, because a failed asset fetch is reported in the webview console,
  // not by the native process. index.html sits at the bundle root, so the
  // two forms resolve identically everywhere this does work.
  base: './',

  define: {
    APP_VERSION: JSON.stringify(packageJson.version)
  },

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
}));
