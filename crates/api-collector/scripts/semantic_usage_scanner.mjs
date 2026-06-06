import fs from "node:fs"
import { createRequire } from "node:module"
import path from "node:path"

const require = createRequire(new URL("../../../npm/package.json", import.meta.url))
const ts = require("typescript")

const input = JSON.parse(fs.readFileSync(0, "utf8"))
const generatedFileName = "__generated_api__.d.ts"
const files = new Map()

for (const file of input.files) {
  files.set(normalizePath(file.path), file.contents)
}
files.set(generatedFileName, renderGeneratedModule(input.package_name, input.endpoints))

const options = {
  allowJs: false,
  exactOptionalPropertyTypes: true,
  module: ts.ModuleKind.NodeNext,
  moduleResolution: ts.ModuleResolutionKind.NodeNext,
  noUncheckedIndexedAccess: true,
  noEmit: true,
  skipLibCheck: true,
  strict: true,
  target: ts.ScriptTarget.ES2022,
}

const defaultHost = ts.createCompilerHost(options, true)
const host = ts.createCompilerHost(options, true)
host.getSourceFile = (fileName, languageVersion) => {
  const normalized = lookupFile(fileName)
  const contents = normalized === undefined ? undefined : files.get(normalized)
  if (contents !== undefined) {
    return ts.createSourceFile(normalized, contents, languageVersion, true)
  }
  return defaultHost.getSourceFile(fileName, languageVersion)
}
host.fileExists = (fileName) => lookupFile(fileName) !== undefined || defaultHost.fileExists(fileName)
host.readFile = (fileName) => {
  const normalized = lookupFile(fileName)
  return normalized === undefined ? defaultHost.readFile(fileName) : files.get(normalized)
}
host.writeFile = () => {}
host.resolveModuleNames = (moduleNames, containingFile) =>
  moduleNames.map((moduleName) => {
    const local = resolveLocalModule(moduleName, containingFile)
    if (local !== undefined) {
      return local
    }
    const resolved = ts.resolveModuleName(moduleName, containingFile, options, defaultHost)
    return resolved.resolvedModule
  })

const program = ts.createProgram([...files.keys()], options, host)
const checker = program.getTypeChecker()
const generatedSource = program.getSourceFile(generatedFileName)

if (generatedSource === undefined) {
  throw new Error("generated API declaration source was not loaded")
}

const endpointTargets = collectEndpointTargets(generatedSource, input.endpoints)
const aliasTargets = collectLocalAliases()
const references = []

for (const source of program.getSourceFiles()) {
  if (source.fileName === generatedFileName || source.isDeclarationFile) {
    continue
  }
  visit(source, (node) => {
    if (!ts.isIdentifier(node) || !isRuntimeReference(node)) {
      return
    }

    const target = endpointForIdentifier(node, aliasTargets, endpointTargets.bySymbol)
    if (target === undefined) {
      return
    }

    const start = source.getLineAndCharacterOfPosition(node.getStart(source))
    const end = source.getLineAndCharacterOfPosition(node.getEnd())
    references.push({
      endpoint_id: target.endpoint_id,
      accessor_path: target.accessor_path,
      file: source.fileName,
      source: {
        file: source.fileName,
        start_line: start.line + 1,
        start_column: start.character + 1,
        end_line: end.line + 1,
        end_column: end.character + 1,
      },
    })
  })
}

references.sort((left, right) =>
  left.endpoint_id.localeCompare(right.endpoint_id) ||
  left.file.localeCompare(right.file) ||
  left.source.start_line - right.source.start_line ||
  left.source.start_column - right.source.start_column
)

process.stdout.write(JSON.stringify({ references }))

function renderGeneratedModule(packageName, endpoints) {
  const namespaces = new Map()
  for (const endpoint of endpoints) {
    const [namespace, functionName] = endpoint.accessor_path
    if (namespace === undefined || functionName === undefined) {
      continue
    }
    const functions = namespaces.get(namespace) ?? []
    functions.push(functionName)
    namespaces.set(namespace, functions)
  }

  const lines = [`declare module ${JSON.stringify(packageName)} {`]
  for (const [namespace, functions] of [...namespaces.entries()].sort(([left], [right]) =>
    left.localeCompare(right)
  )) {
    lines.push(`  export namespace ${namespace} {`)
    for (const functionName of [...new Set(functions)].sort()) {
      lines.push(`    export function ${functionName}(...args: Array<unknown>): unknown`)
    }
    lines.push("  }")
  }
  lines.push("}")
  return lines.join("\n")
}

function collectEndpointTargets(source, endpoints) {
  const endpointByPath = new Map(
    endpoints.map((endpoint) => [endpoint.accessor_path.join("."), endpoint])
  )
  const bySymbol = new Map()
  const namespaceSymbols = new Map()

  visit(source, (node) => {
    if (!ts.isModuleDeclaration(node) || !ts.isIdentifier(node.name)) {
      return
    }
    const namespace = node.name.text
    const namespaceSymbol = resolveSymbol(checker.getSymbolAtLocation(node.name))
    if (namespaceSymbol !== undefined) {
      namespaceSymbols.set(namespaceSymbol, namespace)
    }

    const body = node.body
    if (!body || !ts.isModuleBlock(body)) {
      return
    }
    for (const statement of body.statements) {
      if (!ts.isFunctionDeclaration(statement) || statement.name === undefined) {
        continue
      }
      const endpoint = endpointByPath.get(`${namespace}.${statement.name.text}`)
      if (endpoint === undefined) {
        continue
      }
      const symbol = resolveSymbol(checker.getSymbolAtLocation(statement.name))
      if (symbol !== undefined) {
        bySymbol.set(symbol, endpoint)
      }
    }
  })

  return { bySymbol, namespaceSymbols, endpointByPath }
}

function collectLocalAliases() {
  const aliases = new Map()
  for (const source of program.getSourceFiles()) {
    if (source.fileName === generatedFileName || source.isDeclarationFile) {
      continue
    }
    visit(source, (node) => {
      if (!ts.isVariableDeclaration(node) || node.initializer === undefined) {
        return
      }

      if (ts.isIdentifier(node.name)) {
        const endpoint = endpointForExpression(node.initializer, aliases, endpointTargets)
        if (endpoint !== undefined) {
          const symbol = resolveSymbol(checker.getSymbolAtLocation(node.name))
          if (symbol !== undefined) {
            aliases.set(symbol, endpoint)
          }
        }
      }

      if (ts.isObjectBindingPattern(node.name)) {
        const namespace = namespaceForExpression(node.initializer, endpointTargets.namespaceSymbols)
        if (namespace === undefined) {
          return
        }
        for (const element of node.name.elements) {
          if (!ts.isIdentifier(element.name)) {
            continue
          }
          const propertyName =
            element.propertyName && ts.isIdentifier(element.propertyName)
              ? element.propertyName.text
              : element.name.text
          const endpoint = endpointTargets.endpointByPath.get(`${namespace}.${propertyName}`)
          if (endpoint === undefined) {
            continue
          }
          const symbol = resolveSymbol(checker.getSymbolAtLocation(element.name))
          if (symbol !== undefined) {
            aliases.set(symbol, endpoint)
          }
        }
      }
    })
  }
  return aliases
}

function endpointForExpression(expression, aliases, targets) {
  if (ts.isPropertyAccessExpression(expression)) {
    const symbol = resolveSymbol(checker.getSymbolAtLocation(expression.name))
    return symbol === undefined ? undefined : targets.bySymbol.get(symbol)
  }
  if (ts.isIdentifier(expression)) {
    const symbol = resolveSymbol(checker.getSymbolAtLocation(expression))
    if (symbol === undefined) {
      return undefined
    }
    return aliases.get(symbol) ?? targets.bySymbol.get(symbol)
  }
  return undefined
}

function endpointForIdentifier(identifier, aliases, targetsBySymbol) {
  const symbol = resolveSymbol(checker.getSymbolAtLocation(identifier))
  if (symbol === undefined) {
    return undefined
  }
  return aliases.get(symbol) ?? targetsBySymbol.get(symbol)
}

function namespaceForExpression(expression, namespaceSymbols) {
  if (!ts.isIdentifier(expression)) {
    return undefined
  }
  const symbol = resolveSymbol(checker.getSymbolAtLocation(expression))
  return symbol === undefined ? undefined : namespaceSymbols.get(symbol)
}

function resolveSymbol(symbol) {
  if (symbol === undefined) {
    return undefined
  }
  if ((symbol.flags & ts.SymbolFlags.Alias) !== 0) {
    return checker.getAliasedSymbol(symbol)
  }
  return symbol
}

function isRuntimeReference(identifier) {
  if (isDeclarationName(identifier)) {
    return false
  }
  for (let node = identifier; node !== undefined; node = node.parent) {
    if (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) {
      return false
    }
    if (isTypePosition(node)) {
      return false
    }
  }
  return true
}

function isDeclarationName(identifier) {
  const parent = identifier.parent
  return (
    (ts.isVariableDeclaration(parent) && parent.name === identifier) ||
    (ts.isBindingElement(parent) && parent.name === identifier) ||
    (ts.isFunctionDeclaration(parent) && parent.name === identifier) ||
    (ts.isParameter(parent) && parent.name === identifier) ||
    (ts.isPropertyDeclaration(parent) && parent.name === identifier) ||
    (ts.isMethodDeclaration(parent) && parent.name === identifier) ||
    (ts.isClassDeclaration(parent) && parent.name === identifier) ||
    (ts.isInterfaceDeclaration(parent) && parent.name === identifier) ||
    (ts.isTypeAliasDeclaration(parent) && parent.name === identifier) ||
    (ts.isModuleDeclaration(parent) && parent.name === identifier)
  )
}

function isTypePosition(node) {
  return (
    ts.isTypeNode(node) ||
    ts.isInterfaceDeclaration(node) ||
    ts.isTypeAliasDeclaration(node) ||
    ts.isImportTypeNode(node) ||
    ts.isHeritageClause(node)
  )
}

function visit(node, callback) {
  callback(node)
  ts.forEachChild(node, (child) => visit(child, callback))
}

function normalizePath(path) {
  return path.replaceAll("\\", "/")
}

function lookupFile(fileName) {
  const normalized = normalizePath(fileName)
  if (files.has(normalized)) {
    return normalized
  }

  const relativeToCwd = normalizePath(path.relative(process.cwd(), normalized))
  if (files.has(relativeToCwd)) {
    return relativeToCwd
  }

  return undefined
}

function resolveLocalModule(moduleName, containingFile) {
  if (!moduleName.startsWith(".")) {
    return undefined
  }

  const containingDir = path.posix.dirname(normalizePath(containingFile))
  const requested = normalizePath(path.posix.join(containingDir, moduleName))
  for (const candidate of moduleCandidates(requested)) {
    const resolvedFileName = lookupFile(candidate)
    if (resolvedFileName !== undefined) {
      return {
        resolvedFileName,
        extension: extensionFor(resolvedFileName),
      }
    }
  }

  return undefined
}

function moduleCandidates(requested) {
  const candidates = [requested]
  if (requested.endsWith(".js") || requested.endsWith(".mjs") || requested.endsWith(".cjs")) {
    candidates.push(
      requested.replace(/\.[cm]?js$/, ".ts"),
      requested.replace(/\.[cm]?js$/, ".tsx"),
      requested.replace(/\.[cm]?js$/, ".d.ts")
    )
  }
  candidates.push(
    `${requested}.ts`,
    `${requested}.tsx`,
    `${requested}.d.ts`,
    `${requested}/index.ts`,
    `${requested}/index.tsx`,
    `${requested}/index.d.ts`
  )
  return candidates
}

function extensionFor(fileName) {
  if (fileName.endsWith(".d.ts")) {
    return ts.Extension.Dts
  }
  if (fileName.endsWith(".tsx")) {
    return ts.Extension.Tsx
  }
  if (fileName.endsWith(".jsx")) {
    return ts.Extension.Jsx
  }
  if (fileName.endsWith(".js") || fileName.endsWith(".mjs") || fileName.endsWith(".cjs")) {
    return ts.Extension.Js
  }
  return ts.Extension.Ts
}
