// Packages the vsix from a staged copy outside the npm workspace.
//
// vsce cannot package straight from a workspace member: its npm dependency
// walk resolves the workspace ROOT as the project (pulling sibling packages
// into the vsix), while --no-dependencies unconditionally drops node_modules
// — including the bundled tsserver plugin. A clean copy outside the
// workspace has neither problem.
import { spawnSync } from "node:child_process"
import * as fs from "node:fs"
import { createRequire } from "node:module"
import * as os from "node:os"
import * as path from "node:path"

const extension = path.join(path.dirname(new URL(import.meta.url).pathname), "..")
const staged = fs.mkdtempSync(path.join(os.tmpdir(), "typeweld-vsix-"))

try {
  for (const entry of [
    "dist",
    "bin",
    "LICENSE.md",
    "README.md",
    "node_modules/@typeweld/typescript-plugin",
  ]) {
    fs.cpSync(path.join(extension, entry), path.join(staged, entry), {
      recursive: true,
    })
  }
  // The copy is already built; the staged manifest must not re-run scripts
  // (the staging area has no devDependencies).
  const manifest = JSON.parse(
    fs.readFileSync(path.join(extension, "package.json"), "utf8"),
  )
  delete manifest.scripts
  delete manifest.devDependencies
  fs.writeFileSync(
    path.join(staged, "package.json"),
    JSON.stringify(manifest, null, 2) + "\n",
  )

  const require = createRequire(path.join(extension, "package.json"))
  const vsce = require.resolve("@vscode/vsce/vsce")
  const output = path.join(extension, "typeweld.vsix")
  const result = spawnSync(process.execPath, [vsce, "package", "-o", output], {
    cwd: staged,
    stdio: "inherit",
  })
  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
  console.log(`packaged ${output}`)
} finally {
  fs.rmSync(staged, { force: true, recursive: true })
}
