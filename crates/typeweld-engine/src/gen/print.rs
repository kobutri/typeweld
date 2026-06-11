//! The TypeScript printer.
//!
//! Deterministic formatting (two-space indent, double quotes, no statement
//! semicolons) with byte ranges recorded for every marked node while
//! printing.

use super::ast::{Expr, InterfaceField, Item, MarkRef};

/// One recorded mark: the byte range of a marked node in the printed file.
pub struct PrintedMark {
    pub mark: MarkRef,
    pub start: u32,
    pub end: u32,
}

pub struct Printer {
    out: String,
    marks: Vec<PrintedMark>,
}

impl Printer {
    pub fn new(header: &str) -> Self {
        let mut out = String::new();
        out.push_str(header);
        Self {
            out,
            marks: Vec::new(),
        }
    }

    pub fn finish(self) -> (String, Vec<PrintedMark>) {
        (self.out, self.marks)
    }

    #[allow(clippy::too_many_lines)]
    pub fn item(&mut self, item: &Item) {
        match item {
            Item::Import {
                type_only,
                names,
                from,
            } => {
                let kind = if *type_only { "import type" } else { "import" };
                self.push(&format!("{kind} {{ {} }} from ", names.join(", ")));
                self.string(from);
                self.push("\n");
            }
            Item::ExportStar { from } => {
                self.push("export * from ");
                self.string(from);
                self.push("\n");
            }
            Item::ExportStarAs { alias, from } => {
                self.push(&format!("export * as {alias} from "));
                self.string(from);
                self.push("\n");
            }
            Item::ExportNames { names, from } => {
                self.push(&format!("export {{ {} }} from ", names.join(", ")));
                self.string(from);
                self.push("\n");
            }
            Item::Const {
                doc,
                name,
                mark,
                ty,
                value,
                as_const,
            } => {
                self.doc(doc, 0);
                self.push("export const ");
                self.marked_text(name, mark);
                if let Some(ty) = ty {
                    self.push(&format!(": {ty}"));
                }
                self.push(" = ");
                self.expr(value, 0);
                if *as_const {
                    self.push(" as const");
                }
                self.push("\n");
            }
            Item::FnConst {
                doc,
                name,
                mark,
                params,
                ret,
                body,
            } => {
                self.doc(doc, 0);
                self.push("export const ");
                self.marked_text(name, mark);
                let params = params
                    .iter()
                    .map(|(name, ty)| format!("{name}: {ty}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.push(&format!(" = ({params}): {ret} =>\n  "));
                self.expr(body, 1);
                self.push("\n");
            }
            Item::TypeAlias {
                doc,
                name,
                mark,
                ty,
            } => {
                self.doc(doc, 0);
                self.push("export type ");
                self.marked_text(name, mark);
                self.push(&format!(" = {ty}\n"));
            }
            Item::Interface {
                doc,
                name,
                mark,
                fields,
            } => {
                self.doc(doc, 0);
                self.push("export interface ");
                self.marked_text(name, mark);
                if fields.is_empty() {
                    self.push(" {}\n");
                } else {
                    self.push(" {\n");
                    for field in fields {
                        self.interface_field(field);
                    }
                    self.push("}\n");
                }
            }
            Item::Class {
                doc,
                name,
                mark,
                extends,
            } => {
                self.doc(doc, 0);
                self.push("export class ");
                self.marked_text(name, mark);
                self.push(" extends ");
                self.expr(extends, 0);
                self.push(" {}\n");
            }
            Item::Raw(text) => {
                self.push(text);
                if !text.ends_with('\n') {
                    self.push("\n");
                }
            }
        }
    }

    fn interface_field(&mut self, field: &InterfaceField) {
        self.doc(&field.doc, 1);
        self.push("  readonly ");
        self.marked_text(&field.name, &field.mark);
        if field.optional {
            self.push("?");
        }
        self.push(&format!(": {}\n", field.ty));
    }

    #[allow(clippy::ref_option)]
    fn doc(&mut self, doc: &Option<String>, indent: usize) {
        let Some(doc) = doc else { return };
        let pad = "  ".repeat(indent);
        let lines: Vec<&str> = doc.lines().collect();
        if lines.len() == 1 {
            self.push(&format!("{pad}/** {} */\n", lines[0]));
        } else {
            self.push(&format!("{pad}/**\n"));
            for line in lines {
                self.push(&format!("{pad} * {line}\n"));
            }
            self.push(&format!("{pad} */\n"));
        }
    }

    fn expr(&mut self, expr: &Expr, indent: usize) {
        match expr {
            Expr::Raw(text) => self.push_reindented(text, indent),
            Expr::Str(text) => self.string(text),
            Expr::Member(object, name) => {
                self.expr(object, indent);
                self.push(&format!(".{name}"));
            }
            Expr::Call(callee, args) => {
                self.expr(callee, indent);
                self.push("(");
                let multiline = args
                    .iter()
                    .any(|arg| matches!(arg, Expr::Object(entries) if !entries.is_empty()))
                    || args.iter().any(|arg| matches!(arg, Expr::Arrow { .. }));
                if multiline && args.len() > 1 {
                    let pad = "  ".repeat(indent + 1);
                    for (index, arg) in args.iter().enumerate() {
                        self.push(&format!("\n{pad}"));
                        self.expr(arg, indent + 1);
                        if index + 1 < args.len() {
                            self.push(",");
                        } else {
                            self.push(&format!(",\n{}", "  ".repeat(indent)));
                        }
                    }
                } else {
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            self.push(", ");
                        }
                        self.expr(arg, indent);
                    }
                }
                self.push(")");
            }
            Expr::Object(entries) => {
                if entries.is_empty() {
                    self.push("{}");
                    return;
                }
                self.push("{\n");
                let pad = "  ".repeat(indent + 1);
                for entry in entries {
                    self.push(&pad);
                    self.marked_text(&entry.key, &entry.mark);
                    self.push(": ");
                    self.expr(&entry.value, indent + 1);
                    self.push(",\n");
                }
                self.push(&format!("{}}}", "  ".repeat(indent)));
            }
            Expr::Array(elements) => {
                if elements.len() <= 1 {
                    self.push("[");
                    if let Some(element) = elements.first() {
                        self.expr(element, indent);
                    }
                    self.push("]");
                    return;
                }
                self.push("[\n");
                let pad = "  ".repeat(indent + 1);
                for element in elements {
                    self.push(&pad);
                    self.expr(element, indent + 1);
                    self.push(",\n");
                }
                self.push(&format!("{}]", "  ".repeat(indent)));
            }
            Expr::Arrow { params, body } => {
                self.push(&format!("({}) => ", params.join(", ")));
                if matches!(body.as_ref(), Expr::Object(_)) {
                    self.push("(");
                    self.expr(body, indent);
                    self.push(")");
                } else {
                    self.expr(body, indent);
                }
            }
            Expr::Marked(mark, inner) => {
                let start = self.position();
                self.expr(inner, indent);
                let end = self.position();
                self.marks.push(PrintedMark {
                    mark: mark.clone(),
                    start,
                    end,
                });
            }
        }
    }

    #[allow(clippy::ref_option)]
    fn marked_text(&mut self, text: &str, mark: &Option<MarkRef>) {
        let start = self.position();
        self.push(text);
        if let Some(mark) = mark {
            self.marks.push(PrintedMark {
                mark: mark.clone(),
                start,
                end: self.position(),
            });
        }
    }

    fn string(&mut self, text: &str) {
        self.push("\"");
        for character in text.chars() {
            match character {
                '"' => self.push("\\\""),
                '\\' => self.push("\\\\"),
                '\n' => self.push("\\n"),
                other => {
                    let mut buffer = [0_u8; 4];
                    self.push(other.encode_utf8(&mut buffer));
                }
            }
        }
        self.push("\"");
    }

    /// Pushes raw text, prefixing continuation lines with the current indent.
    fn push_reindented(&mut self, text: &str, indent: usize) {
        let pad = "  ".repeat(indent);
        let mut lines = text.lines();
        if let Some(first) = lines.next() {
            self.push(first);
        }
        for line in lines {
            self.push("\n");
            if !line.is_empty() {
                self.push(&pad);
                self.push(line);
            }
        }
    }

    fn push(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn position(&self) -> u32 {
        u32::try_from(self.out.len()).unwrap_or(u32::MAX)
    }
}
