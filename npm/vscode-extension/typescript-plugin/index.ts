"use strict"

const generatedPackageSegment = "/target/api-contract/effect-v4/packages/"
const logPrefix = "[rust-ts-integration]"

type GeneratedSpan = {
  readonly fileName?: string
  readonly originalFileName?: string
}

type ReferencedSymbol = {
  readonly definition: GeneratedSpan
  readonly references: readonly GeneratedSpan[]
  readonly [key: string]: unknown
}

type DefinitionAndBoundSpan = {
  readonly definitions?: readonly GeneratedSpan[]
  readonly [key: string]: unknown
}

type LanguageServiceMethods = {
  getDefinitionAtPosition?: (
    fileName: string,
    position: number,
  ) => readonly GeneratedSpan[] | undefined
  getDefinitionAndBoundSpan?: (
    fileName: string,
    position: number,
  ) => DefinitionAndBoundSpan | undefined
  getReferencesAtPosition?: (
    fileName: string,
    position: number,
  ) => readonly GeneratedSpan[] | undefined
  findReferences?: (
    fileName: string,
    position: number,
  ) => readonly ReferencedSymbol[] | undefined
  getQuickInfoAtPosition?: (
    fileName: string,
    position: number,
    maximumLength?: number,
  ) => unknown
}

type LanguageService = Record<string, unknown> & LanguageServiceMethods

type PluginCreateInfo = {
  readonly languageService: LanguageService
  readonly project: {
    readonly projectService: {
      readonly logger: {
        readonly info: (message: string) => void
      }
    }
  }
}

function init() {
  return {
    create(info: PluginCreateInfo): LanguageService {
      const languageService = info.languageService
      const proxy: LanguageService = Object.create(null)

      for (const key of Object.keys(languageService)) {
        const value = languageService[key]
        proxy[key] =
          typeof value === "function" ? value.bind(languageService) : value
      }

      const getDefinitionAtPosition = bind(
        languageService,
        "getDefinitionAtPosition",
      )
      const getDefinitionAndBoundSpan = bind(
        languageService,
        "getDefinitionAndBoundSpan",
      )
      const getReferencesAtPosition = bind(
        languageService,
        "getReferencesAtPosition",
      )
      const findReferences = bind(languageService, "findReferences")
      const getQuickInfoAtPosition = bind(
        languageService,
        "getQuickInfoAtPosition",
      )

      if (getDefinitionAtPosition !== undefined) {
        proxy.getDefinitionAtPosition = (fileName, position) => {
          return keepNonGeneratedSpans(
            getDefinitionAtPosition(fileName, position),
          )
        }
      }

      if (getDefinitionAndBoundSpan !== undefined) {
        proxy.getDefinitionAndBoundSpan = (fileName, position) => {
          const result = getDefinitionAndBoundSpan(fileName, position)
          if (result === undefined || result.definitions === undefined) {
            return result
          }

          const definitions = keepNonGeneratedSpans(result.definitions)
          if (definitions === undefined) {
            return undefined
          }

          return definitions.length === result.definitions.length
            ? result
            : { ...result, definitions }
        }
      }

      if (getReferencesAtPosition !== undefined) {
        proxy.getReferencesAtPosition = (fileName, position) => {
          return keepNonGeneratedSpans(
            getReferencesAtPosition(fileName, position),
          )
        }
      }

      if (findReferences !== undefined) {
        proxy.findReferences = (fileName, position) => {
          const symbols = findReferences(fileName, position)
          if (symbols === undefined) {
            return undefined
          }

          const keptSymbols = symbols.flatMap((symbol): ReferencedSymbol[] => {
            if (isGeneratedSpan(symbol.definition)) {
              return []
            }

            const references = symbol.references.filter(
              (reference) => !isGeneratedSpan(reference),
            )
            return references.length === symbol.references.length
              ? [symbol]
              : [{ ...symbol, references }]
          })

          return keptSymbols.length === 0 ? undefined : keptSymbols
        }
      }

      if (
        getQuickInfoAtPosition !== undefined &&
        getDefinitionAtPosition !== undefined
      ) {
        proxy.getQuickInfoAtPosition = (fileName, position, maximumLength) => {
          if (
            hasOnlyGeneratedDefinition(
              getDefinitionAtPosition(fileName, position),
            )
          ) {
            return undefined
          }
          return getQuickInfoAtPosition(fileName, position, maximumLength)
        }
      }

      log(info, "TypeScript server plugin active.")
      return proxy
    },
  }
}

function bind<Method extends keyof LanguageServiceMethods>(
  languageService: LanguageService,
  method: Method,
): NonNullable<LanguageServiceMethods[Method]> | undefined {
  const value = languageService[method]
  return typeof value === "function"
    ? (value.bind(languageService) as NonNullable<LanguageServiceMethods[Method]>)
    : undefined
}

function keepNonGeneratedSpans<Span extends GeneratedSpan>(
  spans: readonly Span[] | undefined,
): Span[] | undefined {
  if (spans === undefined) {
    return undefined
  }

  const kept = spans.filter((span) => !isGeneratedSpan(span))
  return kept.length === 0 && spans.length > 0 ? undefined : kept
}

function hasOnlyGeneratedDefinition(
  spans: readonly GeneratedSpan[] | undefined,
): boolean {
  return (
    spans !== undefined &&
    spans.length > 0 &&
    spans.every((span) => isGeneratedSpan(span))
  )
}

function isGeneratedSpan(span: GeneratedSpan | undefined): boolean {
  return (
    span !== undefined &&
    (isGeneratedPackageFile(span.fileName) ||
      isGeneratedPackageFile(span.originalFileName))
  )
}

function isGeneratedPackageFile(fileName: string | undefined): boolean {
  return (
    typeof fileName === "string" &&
    fileName.replace(/\\/g, "/").includes(generatedPackageSegment)
  )
}

function log(info: PluginCreateInfo, message: string): void {
  try {
    info.project.projectService.logger.info(`${logPrefix} ${message}`)
  } catch {
  }
}

export = init
