import { defineConfig } from "vitest/config"
import path from "node:path"

// The generated package lives outside the app directory, so the runtime and
// effect must resolve to this app's copies for every importer.
const here = (relative: string) => path.resolve(__dirname, relative)

export default defineConfig({
  test: {
    globals: true,
    environment: "node",
  },
  resolve: {
    alias: [
      {
        find: "@workspace/e2e-api",
        replacement: here("../target/typeweld/packages/@workspace/e2e-api"),
      },
      {
        find: "@typeweld/effect-runtime/compat",
        replacement: here("node_modules/@typeweld/effect-runtime/src/compat.ts"),
      },
      {
        find: "@typeweld/effect-runtime",
        replacement: here("node_modules/@typeweld/effect-runtime/src/index.ts"),
      },
      { find: /^effect$/, replacement: here("node_modules/effect") },
      { find: /^effect\/(.*)$/, replacement: here("node_modules/effect") + "/$1" },
    ],
  },
})
