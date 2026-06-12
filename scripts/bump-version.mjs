#!/usr/bin/env node
// Bump every publishable Typeweld package to a new version in one shot.
//
// Usage:
//   node scripts/bump-version.mjs <new-version>
//   node scripts/bump-version.mjs 0.0.6
//
// Updates, keeping diffs minimal:
//   - Cargo.toml          workspace.package version + internal path-dep versions
//   - Cargo.lock          version of each workspace crate (version.workspace = true)
//   - npm/*/package.json  version
//   - npm/package-lock.json  regenerated via `npm install --package-lock-only`
//
// Example crates under examples/ keep their own independent versions.

import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const version = process.argv[2];
if (!version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error('Usage: node scripts/bump-version.mjs <semver>   (e.g. 0.0.6)');
  process.exit(1);
}

const changed = [];

function rewrite(relPath, transform) {
  const path = join(root, relPath);
  const before = readFileSync(path, 'utf8');
  const after = transform(before);
  if (after !== before) {
    writeFileSync(path, after);
    changed.push(relPath);
  }
}

// --- Cargo.toml: the [workspace.package] version and internal path deps. ---
rewrite('Cargo.toml', (text) =>
  text
    // First `version = "..."` after the [workspace.package] header.
    .replace(/(\[workspace\.package\][\s\S]*?\nversion = ")[^"]+(")/, `$1${version}$2`)
    // typeweld* = { path = "crates/typeweld...", version = "..." }
    .replace(/(path = "crates\/typeweld[^"]*", version = ")[^"]+(")/g, `$1${version}$2`),
);

// --- Cargo.lock: every crate that inherits the workspace version. ---
const workspaceCrates = readdirSync(join(root, 'crates'), { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => readFileSync(join(root, 'crates', entry.name, 'Cargo.toml'), 'utf8'))
  .filter((toml) => /version\.workspace\s*=\s*true/.test(toml))
  .map((toml) => toml.match(/^name = "([^"]+)"/m)?.[1])
  .filter(Boolean);

rewrite('Cargo.lock', (text) => {
  let out = text;
  for (const name of workspaceCrates) {
    const re = new RegExp(`(name = "${name}"\\nversion = ")[^"]+(")`);
    out = out.replace(re, `$1${version}$2`);
  }
  return out;
});

// --- npm package.json files. ---
const npmPackages = [
  'npm/typeweld/package.json',
  'npm/effect-runtime/package.json',
  'npm/vscode-extension/package.json',
  'npm/vscode-extension/typescript-plugin/package.json',
];

for (const pkg of npmPackages) {
  rewrite(pkg, (text) =>
    // The package's own version (first "version" key). The launcher's
    // @typeweld/cli-* optionalDependencies are not pinned here; the release
    // workflow injects them via scripts/inject-platform-optional-deps.mjs.
    text.replace(/("version":\s*")[^"]+(")/, `$1${version}$2`),
  );
}

// --- Sync the npm workspace lockfile. ---
try {
  execFileSync('npm', ['install', '--package-lock-only', '--no-audit', '--no-fund'], {
    cwd: join(root, 'npm'),
    stdio: 'inherit',
  });
  changed.push('npm/package-lock.json (if changed)');
} catch (err) {
  console.warn(
    `\nWarning: could not refresh npm/package-lock.json automatically (${err.message}).\n` +
      'Run `npm install --package-lock-only` in npm/ once you have network access.',
  );
}

console.log(`\nBumped to ${version}. Updated:`);
for (const path of changed) console.log(`  - ${path}`);
console.log('\nReview with `git diff`, then commit.');
