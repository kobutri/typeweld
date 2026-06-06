import { resolve } from "node:path"

import { defineConfig } from "vite"

export default defineConfig(({ mode }) => {
  const production = mode === "production"

  return {
    build: {
      emptyOutDir: true,
      minify: production,
      outDir: "dist",
      rollupOptions: {
        external: ["vscode"],
        output: {
          entryFileNames: "extension.js",
          exports: "named",
          format: "cjs",
        },
      },
      sourcemap: !production,
      ssr: resolve("src/extension.ts"),
      target: "node20",
    },
    ssr: {
      external: ["vscode"],
      noExternal: true,
    },
  }
})
