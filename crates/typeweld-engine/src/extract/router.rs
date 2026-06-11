//! `#[api_router]` analysis: which endpoints are mounted where, and the
//! effective route prefixes contributed by `.nest(...)` composition.

use std::collections::{HashMap, HashSet};

use syn::visit::Visit;

use crate::diag::Diagnostic;
use crate::ir::{Endpoint, Span};

use super::modules::span_of;
use super::resolve::{Entity, Resolver};

/// Key for a function item: its module path plus the function name.
type FnKey = (Vec<String>, String);

/// One analyzed `#[api_router]` function.
pub struct RouterFn {
    pub key: FnKey,
    /// Endpoint handlers mounted directly via `.endpoint(handler)`.
    pub mounts: Vec<Mount>,
    /// Child routers from `.nest(prefix, child())` / `.merge(child())`.
    pub children: Vec<(String, FnKey)>,
}

/// One `.endpoint(handler)` call site.
pub struct Mount {
    pub endpoint: FnKey,
    pub span: Span,
}

/// Finds and analyzes every `#[api_router]` function in the walked modules.
pub fn analyze(resolver: &Resolver<'_>, diagnostics: &mut Vec<Diagnostic>) -> Vec<RouterFn> {
    let mut routers = Vec::new();

    let mut module_keys: Vec<&Vec<String>> = resolver.modules.modules.keys().collect();
    module_keys.sort();
    for module_key in module_keys {
        let data = &resolver.modules.modules[module_key];
        for item in &data.items {
            let syn::Item::Fn(item) = item else { continue };
            let has_router_attr = item.attrs.iter().any(|attr| {
                attr.path()
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "api_router")
            });
            if !has_router_attr {
                continue;
            }

            let mut visitor = RouterVisitor {
                resolver,
                module: module_key,
                file: &data.file,
                diagnostics,
                mounts: Vec::new(),
                children: Vec::new(),
            };
            visitor.visit_block(&item.block);
            routers.push(RouterFn {
                key: (module_key.clone(), item.sig.ident.to_string()),
                mounts: visitor.mounts,
                children: visitor.children,
            });
        }
    }

    routers
}

/// Applies router mount information to extracted endpoints: effective routes
/// (with nest prefixes), mount spans, and unmounted-endpoint warnings.
#[allow(clippy::similar_names)]
pub fn apply_mounts(
    routers: &[RouterFn],
    endpoints: &mut [Endpoint],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let by_key: HashMap<&FnKey, &RouterFn> =
        routers.iter().map(|router| (&router.key, router)).collect();
    let referenced: HashSet<&FnKey> = routers
        .iter()
        .flat_map(|router| router.children.iter().map(|(_, child)| child))
        .collect();

    // (endpoint key) -> [(effective route, mount span)]
    let mut mounted: HashMap<FnKey, Vec<(String, Span)>> = HashMap::new();
    for root in routers
        .iter()
        .filter(|router| !referenced.contains(&router.key))
    {
        let mut stack = HashSet::new();
        collect_mounts(root, "", &by_key, &mut mounted, &mut stack);
    }

    for endpoint in endpoints.iter_mut() {
        let module = endpoint.rust_path[..endpoint.rust_path.len() - 1].to_vec();
        let key = (module, endpoint.rust_name.clone());
        let Some(mut mounts) = mounted.remove(&key) else {
            diagnostics.push(Diagnostic::warning(
                "unmounted-endpoint",
                format!(
                    "endpoint `{}` is not mounted in any #[api_router]; it will not be served",
                    endpoint.rust_name
                ),
                Some(
                    endpoint
                        .spans
                        .attr
                        .clone()
                        .unwrap_or_else(|| endpoint.spans.name.clone()),
                ),
            ));
            continue;
        };

        // Each mount records the accumulated nest prefix; the effective route
        // is that prefix plus the path declared in #[api].
        for (prefix, _) in &mut mounts {
            *prefix = join_prefix(prefix, &endpoint.route);
        }
        mounts.sort_by(|a, b| a.0.cmp(&b.0));
        let mut routes: Vec<&String> = mounts.iter().map(|(route, _)| route).collect();
        routes.dedup();
        if routes.len() > 1 {
            diagnostics.push(Diagnostic::warning(
                "ambiguous-mount",
                format!(
                    "endpoint `{}` is mounted at multiple routes ({}); the generated client \
                     uses `{}`",
                    endpoint.rust_name,
                    routes
                        .iter()
                        .map(|route| route.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    routes[0]
                ),
                Some(endpoint.spans.name.clone()),
            ));
        }
        endpoint.route.clone_from(&mounts[0].0);
        endpoint.router_mounts = mounts.into_iter().map(|(_, span)| span).collect();
    }
}

fn collect_mounts<'r>(
    router: &'r RouterFn,
    prefix: &str,
    by_key: &HashMap<&FnKey, &'r RouterFn>,
    mounted: &mut HashMap<FnKey, Vec<(String, Span)>>,
    stack: &mut HashSet<&'r FnKey>,
) {
    if !stack.insert(&router.key) {
        return;
    }

    for mount in &router.mounts {
        mounted
            .entry(mount.endpoint.clone())
            .or_default()
            .push((prefix.to_owned(), mount.span.clone()));
    }
    for (child_prefix, child_key) in &router.children {
        if let Some(child) = by_key.get(child_key) {
            let combined = join_prefix(prefix, child_prefix);
            collect_mounts(child, &combined, by_key, mounted, stack);
        }
    }

    stack.remove(&router.key);
}

fn join_prefix(outer: &str, inner: &str) -> String {
    let outer = outer.trim_end_matches('/');
    let inner = inner.trim_end_matches('/');
    let joined = if inner.is_empty() {
        outer.to_owned()
    } else if inner.starts_with('/') {
        format!("{outer}{inner}")
    } else {
        format!("{outer}/{inner}")
    };
    if joined.is_empty() {
        "/".to_owned()
    } else {
        joined
    }
}

struct RouterVisitor<'a, 'd> {
    resolver: &'a Resolver<'a>,
    module: &'a [String],
    file: &'a str,
    diagnostics: &'d mut Vec<Diagnostic>,
    mounts: Vec<Mount>,
    children: Vec<(String, FnKey)>,
}

impl<'ast> Visit<'ast> for RouterVisitor<'_, '_> {
    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        syn::visit::visit_expr_method_call(self, call);

        match call.method.to_string().as_str() {
            "endpoint" => self.visit_endpoint_call(call),
            "nest" => self.visit_nest_call(call),
            "merge" => self.visit_merge_call(call),
            _ => {}
        }
    }
}

impl RouterVisitor<'_, '_> {
    fn visit_endpoint_call(&mut self, call: &syn::ExprMethodCall) {
        if call.args.len() != 1 {
            self.diagnostics.push(Diagnostic::error(
                "invalid-router-call",
                "#[api_router] .endpoint(...) expects exactly one handler path",
                Some(span_of(self.file, call)),
            ));
            return;
        }
        let handler = call.args.first().expect("length checked");
        let Some(key) = self.resolve_fn(handler) else {
            self.diagnostics.push(Diagnostic::error(
                "invalid-router-call",
                "#[api_router] .endpoint(...) only supports a path to an #[api] handler, like \
                 `get_user` or `users::get_user`",
                Some(span_of(self.file, handler)),
            ));
            return;
        };
        self.mounts.push(Mount {
            endpoint: key,
            span: span_of(self.file, handler),
        });
    }

    fn visit_nest_call(&mut self, call: &syn::ExprMethodCall) {
        if call.args.len() != 2 {
            return;
        }
        let mut args = call.args.iter();
        let prefix = args.next().expect("length checked");
        let child = args.next().expect("length checked");
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(prefix),
            ..
        }) = prefix
        else {
            self.diagnostics.push(Diagnostic::warning(
                "opaque-router-call",
                ".nest(...) with a non-literal prefix is invisible to typeweld; mounted \
                 endpoints will report their unprefixed routes",
                Some(span_of(self.file, prefix)),
            ));
            return;
        };
        if let Some(key) = self.resolve_router_expr(child) {
            self.children.push((prefix.value(), key));
        }
    }

    fn visit_merge_call(&mut self, call: &syn::ExprMethodCall) {
        if call.args.len() != 1 {
            return;
        }
        let child = call.args.first().expect("length checked");
        if let Some(key) = self.resolve_router_expr(child) {
            self.children.push((String::new(), key));
        }
    }

    /// Resolves `routes()` or `module::routes()` to a router function key.
    fn resolve_router_expr(&mut self, expr: &syn::Expr) -> Option<FnKey> {
        let syn::Expr::Call(call) = expr else {
            self.diagnostics.push(Diagnostic::warning(
                "opaque-router-call",
                ".nest(...)/.merge(...) with an expression that is not a direct call to another \
                 #[api_router] function is invisible to typeweld",
                Some(span_of(self.file, expr)),
            ));
            return None;
        };
        let syn::Expr::Path(path) = call.func.as_ref() else {
            return None;
        };
        self.resolve_fn_path(&path.path)
    }

    fn resolve_fn(&mut self, expr: &syn::Expr) -> Option<FnKey> {
        let syn::Expr::Path(path) = expr else {
            return None;
        };
        self.resolve_fn_path(&path.path)
    }

    fn resolve_fn_path(&mut self, path: &syn::Path) -> Option<FnKey> {
        if path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
        {
            return None;
        }
        let segments: Vec<String> = path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        match self.resolver.resolve_path(self.module, &segments) {
            Entity::Item(module, name) => Some((module, name)),
            _ => None,
        }
    }
}
