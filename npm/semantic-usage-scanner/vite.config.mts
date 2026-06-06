import { dirname, basename, resolve } from "node:path"

import { defineConfig } from "vite"

const output = resolve(
  process.env.SEMANTIC_USAGE_SCANNER_OUT ?? "dist/semantic_usage_scanner.js",
)

export default defineConfig({
  build: {
    copyPublicDir: false,
    emptyOutDir: false,
    lib: {
      entry: resolve("src/index.ts"),
      fileName: () => basename(output),
      formats: ["iife"],
      name: "SemanticUsageScanner",
    },
    minify: false,
    outDir: dirname(output),
    rollupOptions: {
      output: {
        banner: "var process = undefined;",
      },
    },
    target: "es2022",
  },
})
