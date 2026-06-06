"use strict"

const generatedPackageSegment = "/target/api-contract/effect-v4/packages/"
const logPrefix = "[rust-ts-integration]"

function init() {
  return {
    create(info) {
      const languageService = info.languageService
      const proxy = Object.create(null)

      for (const key of Object.keys(languageService)) {
        const value = languageService[key]
        proxy[key] = typeof value === "function" ? value.bind(languageService) : value
      }

      const getDefinitionAtPosition = bind(languageService, "getDefinitionAtPosition")
      const getDefinitionAndBoundSpan = bind(languageService, "getDefinitionAndBoundSpan")
      const getReferencesAtPosition = bind(languageService, "getReferencesAtPosition")
      const findReferences = bind(languageService, "findReferences")
      const getQuickInfoAtPosition = bind(languageService, "getQuickInfoAtPosition")

      if (getDefinitionAtPosition !== undefined) {
        proxy.getDefinitionAtPosition = (fileName, position) => {
          return keepNonGeneratedSpans(getDefinitionAtPosition(fileName, position))
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
          return keepNonGeneratedSpans(getReferencesAtPosition(fileName, position))
        }
      }

      if (findReferences !== undefined) {
        proxy.findReferences = (fileName, position) => {
          const symbols = findReferences(fileName, position)
          if (symbols === undefined) {
            return undefined
          }

          const keptSymbols = symbols.flatMap((symbol) => {
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
            hasOnlyGeneratedDefinition(getDefinitionAtPosition(fileName, position))
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

function bind(languageService, method) {
  const value = languageService[method]
  return typeof value === "function" ? value.bind(languageService) : undefined
}

function keepNonGeneratedSpans(spans) {
  if (spans === undefined) {
    return undefined
  }

  const kept = spans.filter((span) => !isGeneratedSpan(span))
  return kept.length === 0 && spans.length > 0 ? undefined : kept
}

function hasOnlyGeneratedDefinition(spans) {
  return (
    spans !== undefined &&
    spans.length > 0 &&
    spans.every((span) => isGeneratedSpan(span))
  )
}

function isGeneratedSpan(span) {
  return (
    span !== undefined &&
    (isGeneratedPackageFile(span.fileName) ||
      isGeneratedPackageFile(span.originalFileName))
  )
}

function isGeneratedPackageFile(fileName) {
  return (
    typeof fileName === "string" &&
    fileName.replace(/\\/g, "/").includes(generatedPackageSegment)
  )
}

function log(info, message) {
  try {
    info.project.projectService.logger.info(`${logPrefix} ${message}`)
  } catch {
  }
}

module.exports = init
