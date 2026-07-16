import { defineConfig, lazyPlugins } from "vite-plus";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: lazyPlugins(async () => {
    const [{ default: react }, { default: tailwindcss }] = await Promise.all([
      import("@vitejs/plugin-react"),
      import("@tailwindcss/vite"),
    ]);
    return [react(), tailwindcss()];
  }),
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
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

  lint: {
    ignorePatterns: ["dist/**", "src-tauri/**"],
    options: {
      typeAware: true,
      typeCheck: true,
    },
  },

  test: {
    include: ["src/**/*.test.{ts,tsx}"],
  },

  staged: {
    "*.{js,jsx,ts,tsx,json,jsonc,css,md,yaml,yml}": "vp check --fix",
  },
});
