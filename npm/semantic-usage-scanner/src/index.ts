// @ts-nocheck
import * as ts from "typescript"

const generatedFileName = "__generated_api__.d.ts"
const effectDeclarationFileName = "/node_modules/effect/dist/index.d.ts"
const typescriptLibDir = "/__typescript_lib__"
const typescriptLibs = Object.fromEntries(
  Object.entries(
    import.meta.glob("../../node_modules/typescript/lib/lib.*.d.ts", {
      eager: true,
      import: "default",
      query: "?raw",
    })
  ).map(([fileName, contents]) => [basename(fileName), contents])
)

let input
let files
let typeById
let errorById
let program
let checker
let endpointTargets
let fieldTargets
let errorTargets
let aliasTargets
let endpointValueAliases

export function scan(inputJson) {
  input = JSON.parse(inputJson)
  files = new Map()
  typeById = new Map((input.types ?? []).map((type) => [type.id, type]))
  errorById = new Map((input.errors ?? []).map((error) => [error.error_id, error]))

  for (const file of input.files) {
    files.set(normalizePath(file.path), file.contents)
  }
  files.set(
    generatedFileName,
    renderGeneratedModule(input.package_name, input.endpoints, input.types, input.errors)
  )
  files.set(effectDeclarationFileName, effectDeclarationSource())

  for (const [fileName, contents] of Object.entries(typescriptLibs)) {
    files.set(`${typescriptLibDir}/${fileName}`, contents)
    files.set(fileName, contents)
  }

  const options = {
    allowJs: false,
    exactOptionalPropertyTypes: true,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    noUncheckedIndexedAccess: true,
    noEmit: true,
    skipLibCheck: true,
    strict: true,
    target: ts.ScriptTarget.ES2022,
  }
  const host = createScannerCompilerHost(options)

  program = ts.createProgram([...files.keys()], options, host)
  checker = program.getTypeChecker()
  const generatedSource = program.getSourceFile(generatedFileName)

  if (generatedSource === undefined) {
    throw new Error("generated API declaration source was not loaded")
  }

  endpointTargets = collectEndpointTargets(generatedSource, input.endpoints)
  fieldTargets = collectGeneratedFieldTargets(generatedSource)
  errorTargets = collectGeneratedErrorTargets(generatedSource)
  aliasTargets = collectLocalAliases()
  endpointValueAliases = collectEndpointValueAliases()
  const references = []
  const errorReferences = []
  const fieldReferences = []

  for (const source of program.getSourceFiles()) {
    if (source.fileName === generatedFileName || source.isDeclarationFile) {
      continue
    }
    visit(source, (node) => {
      if (ts.isStringLiteralLike(node)) {
        for (const target of errorTargetsForStringLiteral(node)) {
          errorReferences.push(symbolReference(source, node, target.symbol_id, target.reason))
        }
        return
      }

      if (!ts.isIdentifier(node) || !isRuntimeReference(node)) {
        return
      }

      for (const target of errorTargetsForIdentifier(node)) {
        errorReferences.push(symbolReference(source, node, target.symbol_id, target.reason))
      }

      const fieldTarget = fieldTargetForIdentifier(node)
      if (fieldTarget !== undefined) {
        fieldReferences.push(fieldReference(source, node, fieldTarget))
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
      references.push({
        endpoint_id: target.endpoint_id,
        accessor_path: target.accessor_path,
        file: source.fileName,
        source: sourceRange(source, node),
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

  return JSON.stringify({
    references,
    error_references: deduplicateReferences(errorReferences),
    field_references: deduplicateReferences(fieldReferences),
    diagnostics: collectProgramDiagnostics(),
  })
}

globalThis.__semanticUsageScanner = { scan }

function renderGeneratedModule(packageName, endpoints, types, errors) {
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
  for (const type of [...types].sort((left, right) => left.ts_name.localeCompare(right.ts_name))) {
    renderGeneratedType(lines, type)
  }
  for (const error of [...errors].sort((left, right) => left.ts_name.localeCompare(right.ts_name))) {
    renderGeneratedError(lines, error)
  }
  for (const [namespace, functions] of [...namespaces.entries()].sort(([left], [right]) =>
    left.localeCompare(right)
  )) {
    lines.push(`  export namespace ${namespace} {`)
    for (const functionName of [...new Set(functions)].sort()) {
      const endpoint = endpoints.find(
        (candidate) => candidate.accessor_path.join(".") === `${namespace}.${functionName}`
      )
      renderGeneratedEndpointArgs(lines, endpoint)
      lines.push(
        `    export const ${functionName}Route: { readonly method: string; readonly path: string }`
      )
      lines.push(
        `    export function ${functionName}(args: ${endpointArgsName(endpoint)}): ${endpointReturnType(endpoint)}`
      )
    }
    lines.push("  }")
  }
  lines.push("}")
  return lines.join("\n")
}

function renderGeneratedType(lines, type) {
  const shape = shapeVariant(type.shape)
  if (shape.kind === "Struct") {
    lines.push(`  export interface ${type.ts_name} {`)
    for (const field of shape.value.fields ?? []) {
      lines.push(
        `    readonly ${propertyName(field.ts_name)}${optionalMarker(field)}: ${typeRef(field.type_ref)}`
      )
    }
    lines.push("  }")
    return
  }

  lines.push(`  export type ${type.ts_name} = ${typeShapeType(type.shape)}`)
}

function renderGeneratedError(lines, error) {
  const classNames = []
  for (const variant of error.variants ?? []) {
    const className = errorVariantClassName(error, variant)
    classNames.push(className)
    lines.push(`  export class ${className} {`)
    lines.push(`    readonly _tag: ${JSON.stringify(variant.tag)}`)
    for (const field of variant.fields ?? []) {
      lines.push(
        `    readonly ${propertyName(field.ts_name)}${optionalMarker(field)}: ${typeRef(field.type_ref)}`
      )
    }
    lines.push("  }")
  }
  lines.push(
    `  export type ${error.ts_name} = ${classNames.length === 0 ? "never" : classNames.join(" | ")}`
  )
}

function renderGeneratedEndpointArgs(lines, endpoint) {
  lines.push(`    export interface ${endpointArgsName(endpoint)} {`)
  for (const field of endpoint?.request?.path_params ?? []) {
    lines.push(
      `      readonly ${propertyName(field.ts_name)}${optionalMarker(field)}: ${typeRef(field.type_ref)}`
    )
  }
  for (const field of endpoint?.request?.query_params ?? []) {
    lines.push(
      `      readonly ${propertyName(field.ts_name)}${optionalMarker(field)}: ${typeRef(field.type_ref)}`
    )
  }
  if (endpoint?.request?.body !== undefined && endpoint.request.body !== null) {
    lines.push(`      readonly body: ${typeRef(endpoint.request.body)}`)
  }
  lines.push("    }")
}

function endpointReturnType(endpoint) {
  const success = endpointSuccessType(endpoint)
  const error = endpointErrorType(endpoint)
  if (endpoint?.transport === "ServerSentEvents") {
    return `import("effect").Stream.Stream<${success}, ${error}, unknown>`
  }
  return `import("effect").Effect.Effect<${success}, ${error}, unknown>`
}

function endpointSuccessType(endpoint) {
  const response = shapeVariant(endpoint?.response)
  if (response.kind === "Json" || response.kind === "Created" || response.kind === "Stream") {
    return typeRef(response.value)
  }
  return "void"
}

function endpointErrorType(endpoint) {
  const names = (endpoint?.errors ?? [])
    .map((errorRef) => errorById.get(errorRef.id)?.ts_name ?? errorRef.name)
    .filter((name) => name !== undefined && name !== "")
  return names.length === 0 ? "never" : [...new Set(names)].join(" | ")
}

function endpointArgsName(endpoint) {
  const functionName = endpoint?.accessor_path?.[1] ?? "api"
  return `${toPascalCase(functionName)}Args`
}

function errorVariantClassName(error, variant) {
  const prefix = error.ts_name.endsWith("Error")
    ? error.ts_name.slice(0, -"Error".length)
    : error.ts_name
  return `${prefix}${variant.rust_name}`
}

function typeShapeType(shape) {
  const variant = shapeVariant(shape)
  switch (variant.kind) {
    case "Primitive":
      return primitiveType(variant.value)
    case "Newtype":
    case "Option":
      return typeRef(variant.value)
    case "List":
      return `ReadonlyArray<${typeRef(variant.value)}>`
    case "Map":
      return `Readonly<Record<${typeRef(variant.value.key)}, ${typeRef(variant.value.value)}>>`
    case "Tuple":
      return `[${(variant.value ?? []).map((item) => typeRef(item)).join(", ")}]`
    case "Enum":
      return "unknown"
    case "External":
      return variant.value?.decoded_ts_name ?? variant.value?.ts_name ?? "unknown"
    default:
      return "unknown"
  }
}

function typeRef(ref) {
  if (ref === undefined || ref === null) {
    return "unknown"
  }
  const type = typeById.get(ref.id)
  if (type !== undefined) {
    return type.ts_name
  }
  return primitiveType(ref.name)
}

function primitiveType(name) {
  switch (name) {
    case "String":
    case "str":
      return "string"
    case "bool":
      return "boolean"
    case "i64":
    case "u64":
    case "i128":
    case "u128":
      return "bigint"
    case "i8":
    case "i16":
    case "i32":
    case "u8":
    case "u16":
    case "u32":
    case "f32":
    case "f64":
      return "number"
    case "()":
      return "void"
    default:
      return "unknown"
  }
}

function optionalMarker(field) {
  return field.optionality === "Optional" || field.optionality === "OptionalNullable" ? "?" : ""
}

function propertyName(name) {
  return isIdentifierText(name) ? name : JSON.stringify(name)
}

function isIdentifierText(value) {
  return /^[$A-Z_a-z][$\w]*$/.test(value)
}

function shapeVariant(shape) {
  if (shape === undefined || shape === null || typeof shape !== "object") {
    return { kind: undefined, value: undefined }
  }
  const [kind, value] = Object.entries(shape)[0] ?? []
  return { kind, value }
}

function toPascalCase(value) {
  let output = ""
  let uppercaseNext = true
  for (const character of value) {
    if (character === "_" || character === "-" || character === " ") {
      uppercaseNext = true
      continue
    }
    output += uppercaseNext ? character.toUpperCase() : character
    uppercaseNext = false
  }
  return output
}

function collectEndpointTargets(source, endpoints) {
  const endpointByPath = new Map(
    endpoints.flatMap((endpoint) => {
      const [namespace, functionName] = endpoint.accessor_path
      return [
        [endpoint.accessor_path.join("."), endpoint],
        [`${namespace}.${functionName}Route`, endpoint],
      ]
    })
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
        if (!ts.isVariableStatement(statement)) {
          continue
        }
        for (const declaration of statement.declarationList.declarations) {
          if (!ts.isIdentifier(declaration.name)) {
            continue
          }
          const endpoint = endpointByPath.get(`${namespace}.${declaration.name.text}`)
          if (endpoint === undefined) {
            continue
          }
          const symbol = resolveSymbol(checker.getSymbolAtLocation(declaration.name))
          if (symbol !== undefined) {
            bySymbol.set(symbol, endpoint)
          }
        }
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

function collectGeneratedFieldTargets(source) {
  const targetByOwnerProperty = generatedFieldTargetByOwnerProperty()
  const bySymbol = new Map()

  visit(source, (node) => {
    if (
      (!ts.isPropertySignature(node) && !ts.isPropertyDeclaration(node)) ||
      node.name === undefined
    ) {
      return
    }
    const ownerName = node.parent?.name?.text
    const property = propertyNameText(node.name)
    if (ownerName === undefined || property === undefined) {
      return
    }
    const target = targetByOwnerProperty.get(`${ownerName}.${property}`)
    if (target === undefined) {
      return
    }
    const symbol = resolveSymbol(checker.getSymbolAtLocation(node.name))
    if (symbol !== undefined) {
      bySymbol.set(symbol, target)
    }
  })

  return { bySymbol, targetByOwnerProperty }
}

function collectGeneratedErrorTargets(source) {
  const byClassSymbol = new Map()
  const byTag = new Map()

  for (const error of input.errors ?? []) {
    for (const variant of error.variants ?? []) {
      const targets = [
        {
          symbol_id: variant.variant_id,
          reason: `Effect handler catches ${variant.tag}`,
        },
        {
          symbol_id: variant.tag_symbol_id,
          reason: `Effect handler references error tag ${variant.tag}`,
        },
      ]
      byTag.set(variant.tag, targets)
    }
  }

  visit(source, (node) => {
    if (!ts.isClassDeclaration(node) || node.name === undefined) {
      return
    }
    const target = generatedErrorVariantByClassName().get(node.name.text)
    if (target === undefined) {
      return
    }
    const symbol = resolveSymbol(checker.getSymbolAtLocation(node.name))
    if (symbol !== undefined) {
      byClassSymbol.set(symbol, target)
    }
  })

  return { byClassSymbol, byTag }
}

function generatedFieldTargetByOwnerProperty() {
  const targets = new Map()
  for (const type of input.types ?? []) {
    const shape = shapeVariant(type.shape)
    if (shape.kind !== "Struct") {
      continue
    }
    for (const field of shape.value.fields ?? []) {
      targets.set(`${type.ts_name}.${field.ts_name}`, {
        field_id: field.id,
        access: undefined,
        reason: `generated DTO field ${type.ts_name}.${field.ts_name}`,
      })
    }
  }
  for (const error of input.errors ?? []) {
    for (const variant of error.variants ?? []) {
      const className = errorVariantClassName(error, variant)
      for (const field of variant.fields ?? []) {
        targets.set(`${className}.${field.ts_name}`, {
          field_id: field.id,
          access: undefined,
          reason: `generated error field ${className}.${field.ts_name}`,
        })
      }
    }
  }
  for (const endpoint of input.endpoints ?? []) {
    const argsName = endpointArgsName(endpoint)
    for (const field of endpoint.request?.path_params ?? []) {
      targets.set(`${argsName}.${field.ts_name}`, {
        field_id: field.id,
        access: undefined,
        reason: `generated endpoint path argument ${argsName}.${field.ts_name}`,
      })
    }
    for (const field of endpoint.request?.query_params ?? []) {
      targets.set(`${argsName}.${field.ts_name}`, {
        field_id: field.id,
        access: undefined,
        reason: `generated endpoint query argument ${argsName}.${field.ts_name}`,
      })
    }
  }
  return targets
}

function generatedErrorVariantByClassName() {
  const targets = new Map()
  for (const error of input.errors ?? []) {
    for (const variant of error.variants ?? []) {
      targets.set(errorVariantClassName(error, variant), {
        symbol_id: variant.variant_id,
        reason: `generated error class ${errorVariantClassName(error, variant)} is referenced`,
      })
    }
  }
  return targets
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

function fieldTargetForIdentifier(identifier) {
  const direct = fieldTargetForPropertyAccess(identifier)
  if (direct !== undefined) {
    return direct
  }
  return fieldTargetForObjectLiteralProperty(identifier)
}

function fieldTargetForPropertyAccess(identifier) {
  if (
    !ts.isPropertyAccessExpression(identifier.parent) ||
    identifier.parent.name !== identifier
  ) {
    return undefined
  }
  const symbol = resolveSymbol(checker.getSymbolAtLocation(identifier))
  const target = symbol === undefined ? undefined : fieldTargets.bySymbol.get(symbol)
  if (target === undefined) {
    return undefined
  }
  return {
    ...target,
    access: propertyAccessIsWrite(identifier.parent) ? "Write" : "Read",
  }
}

function fieldTargetForObjectLiteralProperty(identifier) {
  const parent = identifier.parent
  if (
    !ts.isPropertyAssignment(parent) &&
    !ts.isShorthandPropertyAssignment(parent)
  ) {
    return undefined
  }
  if (parent.name !== identifier) {
    return undefined
  }
  const objectLiteral = parent.parent
  if (!ts.isObjectLiteralExpression(objectLiteral)) {
    return undefined
  }
  const contextualType = checker.getContextualType(objectLiteral)
  const property = contextualType?.getProperty(identifier.text)
  const target =
    property === undefined ? undefined : fieldTargets.bySymbol.get(resolveSymbol(property))
  if (target === undefined) {
    return undefined
  }
  return {
    ...target,
    access: "Write",
    reason: `${target.reason} is provided in an object literal`,
  }
}

function errorTargetsForIdentifier(identifier) {
  if (isCatchTagsIdentifierProperty(identifier)) {
    return errorTargets.byTag.get(identifier.text) ?? []
  }
  const symbol = resolveSymbol(checker.getSymbolAtLocation(identifier))
  const target = symbol === undefined ? undefined : errorTargets.byClassSymbol.get(symbol)
  return target === undefined ? [] : [target]
}

function errorTargetsForStringLiteral(literal) {
  if (!isCatchTagLiteral(literal) && !isCatchTagsProperty(literal)) {
    return []
  }
  return errorTargets.byTag.get(literal.text) ?? []
}

function isCatchTagLiteral(literal) {
  const call = literal.parent
  return (
    ts.isCallExpression(call) &&
    call.arguments[0] === literal &&
    isEffectCatchCall(call, "catchTag")
  )
}

function isCatchTagsProperty(literal) {
  const parent = literal.parent
  if (!ts.isPropertyAssignment(parent) || parent.name !== literal) {
    return false
  }
  const objectLiteral = parent.parent
  if (!ts.isObjectLiteralExpression(objectLiteral)) {
    return false
  }
  const call = objectLiteral.parent
  return (
    ts.isCallExpression(call) &&
    call.arguments[0] === objectLiteral &&
    isEffectCatchCall(call, "catchTags")
  )
}

function isCatchTagsIdentifierProperty(identifier) {
  const parent = identifier.parent
  if (!ts.isPropertyAssignment(parent) || parent.name !== identifier) {
    return false
  }
  const objectLiteral = parent.parent
  if (!ts.isObjectLiteralExpression(objectLiteral)) {
    return false
  }
  const call = objectLiteral.parent
  return (
    ts.isCallExpression(call) &&
    call.arguments[0] === objectLiteral &&
    isEffectCatchCall(call, "catchTags")
  )
}

function isEffectCatchCall(call, name) {
  if (
    !ts.isPropertyAccessExpression(call.expression) ||
    call.expression.name.text !== name
  ) {
    return false
  }
  return isEffectModuleSymbol(call.expression.name)
}

function isEffectModuleSymbol(identifier) {
  const symbol = resolveSymbol(checker.getSymbolAtLocation(identifier))
  const declarations = symbol?.declarations ?? []
  return declarations.some((declaration) =>
    normalizePath(declaration.getSourceFile().fileName).includes("/node_modules/effect/")
  )
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

function symbolReference(source, node, symbolId, reason) {
  return {
    symbol_id: symbolId,
    file: source.fileName,
    source: sourceRange(source, node),
    reason,
  }
}

function fieldReference(source, node, target) {
  return {
    field_id: target.field_id,
    file: source.fileName,
    source: sourceRange(source, node),
    access: target.access ?? "Read",
    reason: target.reason,
  }
}

function sourceRange(source, node) {
  const start = source.getLineAndCharacterOfPosition(node.getStart(source))
  const end = source.getLineAndCharacterOfPosition(node.getEnd())
  return {
    file: source.fileName,
    start_line: start.line + 1,
    start_column: start.character + 1,
    end_line: end.line + 1,
    end_column: end.character + 1,
  }
}

function deduplicateReferences(references) {
  const seen = new Set()
  const output = []
  for (const reference of references) {
    const symbolId = reference.symbol_id ?? reference.field_id
    const key = [
      symbolId,
      reference.file,
      reference.source.start_line,
      reference.source.start_column,
      reference.source.end_line,
      reference.source.end_column,
      reference.access ?? "",
    ].join("\0")
    if (seen.has(key)) {
      continue
    }
    seen.add(key)
    output.push(reference)
  }
  return output.sort(
    (left, right) =>
      (left.symbol_id ?? left.field_id).localeCompare(right.symbol_id ?? right.field_id) ||
      left.file.localeCompare(right.file) ||
      left.source.start_line - right.source.start_line ||
      left.source.start_column - right.source.start_column
  )
}

function propertyNameText(name) {
  if (ts.isIdentifier(name) || ts.isStringLiteralLike(name) || ts.isNumericLiteral(name)) {
    return name.text
  }
  return undefined
}

function propertyAccessIsWrite(expression) {
  const parent = expression.parent
  if (
    ts.isBinaryExpression(parent) &&
    parent.left === expression &&
    ts.isAssignmentOperator(parent.operatorToken.kind)
  ) {
    return true
  }
  return (
    (ts.isPrefixUnaryExpression(parent) || ts.isPostfixUnaryExpression(parent)) &&
    parent.operand === expression &&
    (parent.operator === ts.SyntaxKind.PlusPlusToken ||
      parent.operator === ts.SyntaxKind.MinusMinusToken)
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

function createScannerCompilerHost(options) {
  return {
    getSourceFile: (fileName, languageVersion) => {
      const normalized = lookupFile(fileName)
      const contents = normalized === undefined ? undefined : files.get(normalized)
      if (contents === undefined) {
        return undefined
      }
      return ts.createSourceFile(normalized, contents, languageVersion, true)
    },
    getDefaultLibFileName: () => `${typescriptLibDir}/lib.es2022.full.d.ts`,
    getDefaultLibLocation: () => typescriptLibDir,
    getCurrentDirectory: () => "/",
    getCanonicalFileName: (fileName) => normalizePath(fileName),
    useCaseSensitiveFileNames: () => true,
    getNewLine: () => "\n",
    fileExists: (fileName) => lookupFile(fileName) !== undefined,
    readFile: (fileName) => {
      const normalized = lookupFile(fileName)
      return normalized === undefined ? undefined : files.get(normalized)
    },
    writeFile: () => {},
    directoryExists: (directoryName) => {
      const normalized = normalizePath(directoryName).replace(/\/+$/, "")
      return [...files.keys()].some((fileName) => fileName.startsWith(`${normalized}/`))
    },
    getDirectories: () => [],
    realpath: (fileName) => normalizePath(fileName),
    resolveModuleNames: (moduleNames, containingFile) =>
      moduleNames.map((moduleName) => {
        const local = resolveLocalModule(moduleName, containingFile)
        if (local !== undefined) {
          return local
        }
        if (moduleName === "effect") {
          return {
            resolvedFileName: effectDeclarationFileName,
            extension: ts.Extension.Dts,
          }
        }
        return undefined
      }),
  }
}

function effectDeclarationSource() {
  const errorForTag = generatedEffectErrorForTagType()
  return `
declare module "effect" {
  type __RustTsIntegrationErrorForTag<Tag extends string> = ${errorForTag}

  export namespace Effect {
    export interface Effect<A = unknown, E = unknown, R = unknown> {
      pipe<B>(ab: (self: Effect<A, E, R>) => B): B
      pipe<B, C>(ab: (self: Effect<A, E, R>) => B, bc: (self: B) => C): C
      pipe<B, C, D>(
        ab: (self: Effect<A, E, R>) => B,
        bc: (self: B) => C,
        cd: (self: C) => D,
      ): D
      [Symbol.iterator](): Iterator<Effect<A, E, R>, A, unknown>
    }
    export function all(input: unknown): Effect<unknown, unknown, unknown>
    export function catchTag<Tag extends string>(
      tag: Tag,
      handler: (error: __RustTsIntegrationErrorForTag<Tag>) => Effect<unknown, unknown, unknown>,
    ): <A, E, R>(effect: Effect<A, E, R>) => Effect<A, E, R>
    export function catchTags<Handlers extends Record<string, (error: any) => Effect<unknown, unknown, unknown>>>(
      handlers: { readonly [Tag in keyof Handlers]: (error: __RustTsIntegrationErrorForTag<Extract<Tag, string>>) => Effect<unknown, unknown, unknown> },
    ): <A, E, R>(effect: Effect<A, E, R>) => Effect<A, E, R>
    export function gen(body: (...args: ReadonlyArray<unknown>) => Generator<unknown, unknown, unknown>): Effect<unknown, unknown, unknown>
    export function retry(options: unknown): <A, E, R>(effect: Effect<A, E, R>) => Effect<A, E, R>
    export function succeed<A>(value: A): Effect<A, never, never>
  }

  export namespace Layer {
    export interface Layer<A = unknown, E = unknown, R = unknown> {
      pipe(...args: ReadonlyArray<unknown>): Layer<unknown, unknown, unknown>
    }
    export function effect(tag: unknown, effect: Effect.Effect<unknown, unknown, unknown>): Layer<unknown, unknown, unknown>
  }

  export namespace Stream {
    export interface Stream<A = unknown, E = unknown, R = unknown> {
      pipe(...args: ReadonlyArray<unknown>): Stream<unknown, unknown, unknown>
    }
    export function fromEffect(effect: Effect.Effect<unknown, unknown, unknown>): Stream<unknown, unknown, unknown>
  }
}
`
}

function generatedEffectErrorForTagType() {
  const packageName = JSON.stringify(input.package_name)
  const branches = []
  for (const error of input.errors ?? []) {
    for (const variant of error.variants ?? []) {
      branches.push(
        `Tag extends ${JSON.stringify(variant.tag)} ? import(${packageName}).${errorVariantClassName(error, variant)} :`
      )
    }
  }
  return `${branches.join(" ")} never`
}

function normalizePath(path) {
  return path.replaceAll("\\", "/")
}

function basename(path) {
  return normalizePath(path).split("/").pop()
}

function lookupFile(fileName) {
  const normalized = normalizePath(fileName)
  if (files.has(normalized)) {
    return normalized
  }

  const withoutLeadingSlash = normalized.replace(/^\/+/, "")
  if (files.has(withoutLeadingSlash)) {
    return withoutLeadingSlash
  }

  const withLeadingSlash = `/${withoutLeadingSlash}`
  if (files.has(withLeadingSlash)) {
    return withLeadingSlash
  }

  return undefined
}

function resolveLocalModule(moduleName, containingFile) {
  if (!moduleName.startsWith(".")) {
    return undefined
  }

  const containingDir = posixDirname(normalizePath(containingFile))
  const requested = normalizePath(posixJoin(containingDir, moduleName))
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

function posixDirname(fileName) {
  const normalized = normalizePath(fileName).replace(/\/+$/, "")
  const index = normalized.lastIndexOf("/")
  if (index < 0) {
    return "."
  }
  if (index === 0) {
    return "/"
  }
  return normalized.slice(0, index)
}

function posixJoin(base, requested) {
  if (requested.startsWith("/")) {
    return requested
  }
  const parts = `${base === "." ? "" : base}/${requested}`.split("/")
  const output = []
  for (const part of parts) {
    if (part === "" || part === ".") {
      continue
    }
    if (part === "..") {
      output.pop()
      continue
    }
    output.push(part)
  }
  return `${base.startsWith("/") ? "/" : ""}${output.join("/")}`
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
