//! Extraction and lint diagnostics.

use std::fmt;

use crate::ir::Span;
use crate::line_index::LineIndex;

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

/// One diagnostic produced by extraction or linting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable machine-readable code, e.g. `unresolved-type`.
    pub code: &'static str,
    pub message: String,
    /// Span in workspace sources; `None` for workspace-level problems.
    pub span: Option<Span>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: &'static str, message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            span,
        }
    }

    #[must_use]
    pub fn warning(code: &'static str, message: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            span,
        }
    }

    /// Renders `file:line:col: severity[code]: message` using the file
    /// contents to resolve the span position.
    #[must_use]
    pub fn render(&self, source: Option<&str>) -> String {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        match (&self.span, source) {
            (Some(span), Some(source)) => {
                let index = LineIndex::new(source);
                let (line, col) = index.line_col(span.start);
                format!(
                    "{}:{}:{}: {severity}[{}]: {}",
                    span.file,
                    line + 1,
                    col + 1,
                    self.code,
                    self.message
                )
            }
            (Some(span), None) => {
                format!("{}: {severity}[{}]: {}", span.file, self.code, self.message)
            }
            (None, _) => format!("{severity}[{}]: {}", self.code, self.message),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(None))
    }
}
