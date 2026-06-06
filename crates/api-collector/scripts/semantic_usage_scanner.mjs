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
const endpointValueAliases = collectEndpointValueAliases()
const references = []

for (const source of program.getSourceFiles()) {
  if (source.fileName === generatedFileName || source.isDeclarationFile) {
    continue
  }
  visit(source, (node) => {
    if (!ts.isIdentifier(node) || !isRuntimeReference(node)) {
      return
    }

    const accessorTarget = endpointForIdentifier(node, aliasTargets, endpointTargets.bySymbol)
    const valueTarget = endpointValueForIdentifier(node)
    const target = accessorTarget ?? valueTarget
    if (target === undefined) {
      return
    }

    const classification =
      accessorTarget !== undefined
        ? classifyEndpointAccessorReference(node)
        : classifyEndpointValueReference(node)
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
      strength: classification.strength,
      reason: classification.reason,
    })
  })
}

references.sort((left, right) =>
  left.endpoint_id.localeCompare(right.endpoint_id) ||
  left.file.localeCompare(right.file) ||
  left.source.start_line - right.source.start_line ||
  left.source.start_column - right.source.start_column
)

process.stdout.write(JSON.stringify({ references, diagnostics: collectProgramDiagnostics() }))

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
      const endpoint = endpoints.find(
        (candidate) => candidate.accessor_path.join(".") === `${namespace}.${functionName}`
      )
      lines.push(
        `    export function ${functionName}(...args: Array<unknown>): ${endpointReturnType(endpoint)}`
      )
    }
    lines.push("  }")
  }
  lines.push("}")
  return lines.join("\n")
}

function endpointReturnType(endpoint) {
  if (endpoint?.transport === "ServerSentEvents") {
    return 'import("effect").Stream.Stream<unknown, unknown, unknown>'
  }
  return 'import("effect").Effect.Effect<unknown, unknown, unknown>'
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

function collectEndpointValueAliases() {
  const aliases = new Map()
  for (const source of program.getSourceFiles()) {
    if (source.fileName === generatedFileName || source.isDeclarationFile) {
      continue
    }
    visit(source, (node) => {
      if (
        !ts.isVariableDeclaration(node) ||
        !ts.isIdentifier(node.name) ||
        node.initializer === undefined
      ) {
        return
      }

      const target = endpointTargetForCallExpression(node.initializer)
      if (target === undefined || endpointRuntimeKind(node.initializer) === undefined) {
        return
      }

      const symbol = resolveSymbol(checker.getSymbolAtLocation(node.name))
      if (symbol !== undefined) {
        aliases.set(symbol, target)
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

function endpointValueForIdentifier(identifier) {
  const symbol = resolveSymbol(checker.getSymbolAtLocation(identifier))
  return symbol === undefined ? undefined : endpointValueAliases.get(symbol)
}

function endpointTargetForCallExpression(expression) {
  if (!ts.isCallExpression(expression)) {
    return undefined
  }
  return endpointForExpression(expression.expression, aliasTargets, endpointTargets)
}

function classifyEndpointAccessorReference(identifier) {
  const expression = endpointReferenceExpression(identifier)
  const call = endpointCallExpression(expression)
  if (call === undefined) {
    return {
      strength: "Weak",
      reason: "endpoint accessor is referenced without being invoked",
    }
  }

  return classifyEndpointResultExpression(call)
}

function classifyEndpointValueReference(identifier) {
  return classifyEndpointResultExpression(endpointReferenceExpression(identifier))
}

function endpointReferenceExpression(identifier) {
  if (
    ts.isPropertyAccessExpression(identifier.parent) &&
    identifier.parent.name === identifier
  ) {
    return identifier.parent
  }
  return identifier
}

function endpointCallExpression(expression) {
  const outer = outerExpression(expression)
  if (ts.isCallExpression(outer.parent) && outer.parent.expression === outer) {
    return outer.parent
  }
  return undefined
}

function classifyEndpointResultExpression(expression) {
  const kind = endpointRuntimeKind(expression)
  if (kind === undefined) {
    return {
      strength: "Unknown",
      reason: "endpoint invocation could not be verified as an Effect or Stream value",
    }
  }

  const outer = outerExpression(expression)
  const parent = outer.parent

  if (ts.isYieldExpression(parent) && parent.expression === outer) {
    return kind === "Effect"
      ? {
          strength: "Strong",
          reason: "endpoint Effect is yielded",
        }
      : {
          strength: "Unknown",
          reason: `endpoint ${kind} is yielded, but only Effect values are live in generators`,
        }
  }

  if (ts.isReturnStatement(parent) && parent.expression === outer) {
    return {
      strength: "Strong",
      reason: `endpoint ${kind} is returned`,
    }
  }

  if (ts.isArrowFunction(parent) && parent.body === outer) {
    return {
      strength: "Strong",
      reason: `endpoint ${kind} is returned from an arrow function`,
    }
  }

  const pipe = enclosingPipeCall(outer)
  if (pipe !== undefined && endpointRuntimeKind(pipe) !== undefined) {
    return {
      strength: "Strong",
      reason: `endpoint ${kind} is composed with pipe`,
    }
  }

  const combinator = enclosingLiveCombinatorCall(outer)
  if (combinator !== undefined) {
    return {
      strength: "Strong",
      reason: `endpoint ${kind} is passed to ${combinator}`,
    }
  }

  if (ts.isVoidExpression(parent) && parent.expression === outer) {
    return {
      strength: "Invalid",
      reason: `endpoint ${kind} is explicitly discarded`,
    }
  }

  return {
    strength: "Weak",
    reason: `endpoint ${kind} is invoked without being yielded, returned, or composed`,
  }
}

function endpointRuntimeKind(expression) {
  const type = checker.getTypeAtLocation(expression)
  return runtimeKindForType(type, expression)
}

function runtimeKindForType(type, node) {
  const symbolName = type.getSymbol()?.name ?? type.aliasSymbol?.name
  if (symbolName === "Effect" || symbolName === "Stream" || symbolName === "Layer") {
    return symbolName
  }

  const rendered = checker.typeToString(
    type,
    node,
    ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.UseFullyQualifiedType
  )
  if (/\bEffect(?:\.Effect)?</.test(rendered)) {
    return "Effect"
  }
  if (/\bStream(?:\.Stream)?</.test(rendered)) {
    return "Stream"
  }
  if (/\bLayer(?:\.Layer)?</.test(rendered)) {
    return "Layer"
  }
  return undefined
}

function outerExpression(expression) {
  let node = expression
  while (ts.isParenthesizedExpression(node.parent) && node.parent.expression === node) {
    node = node.parent
  }
  return node
}

function enclosingPipeCall(expression) {
  const parent = expression.parent
  if (
    ts.isPropertyAccessExpression(parent) &&
    parent.expression === expression &&
    parent.name.text === "pipe" &&
    ts.isCallExpression(parent.parent) &&
    parent.parent.expression === parent
  ) {
    return parent.parent
  }
  return undefined
}

function enclosingLiveCombinatorCall(expression) {
  for (let node = expression.parent; node !== undefined; node = node.parent) {
    if (isFunctionBoundary(node)) {
      return undefined
    }
    if (!ts.isCallExpression(node) || node === expression) {
      continue
    }
    if (!node.arguments.some((argument) => containsNode(argument, expression))) {
      continue
    }

    const name = dottedName(node.expression)
    if (
      name !== "Effect.all" &&
      name !== "Layer.effect" &&
      name !== "Stream.fromEffect"
    ) {
      continue
    }

    if (endpointRuntimeKind(node) !== undefined) {
      return name
    }
  }
  return undefined
}

function dottedName(expression) {
  if (ts.isIdentifier(expression)) {
    return expression.text
  }
  if (ts.isPropertyAccessExpression(expression)) {
    const base = dottedName(expression.expression)
    return base === undefined ? undefined : `${base}.${expression.name.text}`
  }
  return undefined
}

function containsNode(root, needle) {
  if (root === needle) {
    return true
  }
  let found = false
  ts.forEachChild(root, (child) => {
    if (!found && containsNode(child, needle)) {
      found = true
    }
  })
  return found
}

function isFunctionBoundary(node) {
  return (
    ts.isFunctionDeclaration(node) ||
    ts.isFunctionExpression(node) ||
    ts.isArrowFunction(node) ||
    ts.isMethodDeclaration(node) ||
    ts.isConstructorDeclaration(node)
  )
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

function collectProgramDiagnostics() {
  return [...program.getSyntacticDiagnostics(), ...program.getSemanticDiagnostics()]
    .map((diagnostic) => diagnosticToJson(diagnostic))
    .filter((diagnostic) => diagnostic !== undefined)
}

function diagnosticToJson(diagnostic) {
  if (
    diagnostic.file === undefined ||
    diagnostic.start === undefined ||
    diagnostic.length === undefined
  ) {
    return undefined
  }

  const start = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start)
  const end = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start + diagnostic.length)
  return {
    code: String(diagnostic.code),
    message: ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
    source: {
      file: diagnostic.file.fileName,
      start_line: start.line + 1,
      start_column: start.character + 1,
      end_line: end.line + 1,
      end_column: end.character + 1,
    },
  }
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
