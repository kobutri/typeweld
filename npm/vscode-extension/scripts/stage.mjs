// Stages the files vsce packages but npm does not place here:
// - a physical copy of @typeweld/typescript-plugin (npm workspaces hoist the
//   package to the npm root, so the extension's own node_modules is empty),
// - the bin/ directory (release builds drop platform binaries in; local
//   builds get a placeholder so the files pattern matches).
import * as fs from "node:fs"
import * as path from "node:path"
import { fileURLToPath } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))
const extension = path.join(here, "..")

const staged = path.join(extension, "node_modules", "@typeweld", "typescript-plugin")
fs.rmSync(staged, { force: true, recursive: true })
fs.mkdirSync(path.join(staged, "dist"), { recursive: true })
for (const file of ["package.json", "dist/index.js"]) {
  fs.copyFileSync(
    path.join(extension, "typescript-plugin", file),
    path.join(staged, file),
  )
}

const bin = path.join(extension, "bin")
fs.mkdirSync(bin, { recursive: true })
if (fs.readdirSync(bin).length === 0) {
  fs.writeFileSync(
    path.join(bin, "README.txt"),
    "Platform binaries are bundled here by release builds; development\nbuilds resolve the typeweld binary from PATH or cargo target dirs.\n",
  )
}
