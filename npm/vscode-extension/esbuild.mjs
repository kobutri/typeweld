import { rmSync } from "node:fs"

import * as esbuild from "esbuild"

const production = process.argv.includes("--production")
const watch = process.argv.includes("--watch")

const options = {
  bundle: true,
  entryPoints: ["src/extension.ts"],
  external: ["vscode"],
  format: "cjs",
  legalComments: "eof",
  logLevel: "info",
  minify: production,
  outfile: "dist/extension.js",
  platform: "node",
  sourcemap: !production,
  target: "node20",
}

rmSync("dist", { force: true, recursive: true })

if (watch) {
  const context = await esbuild.context(options)
  await context.watch()
  console.log("Watching VS Code extension bundle.")
} else {
  await esbuild.build(options)
}
