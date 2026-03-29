use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    visit_mut::{self, VisitMut},
    Expr, ExprCall, ExprPath, ItemFn, Path,
};

/// Rewrites every `tokio::spawn(...)` call in a function body to
/// `replay_core::spawn_with_ctx(...)` so that the ambient [`MockContext`] is
/// propagated into spawned tasks automatically.
pub fn expand_instrument_spawns(mut func: ItemFn) -> TokenStream {
    let mut rewriter = SpawnRewriter;
    rewriter.visit_item_fn_mut(&mut func);
    quote! { #func }
}

// ── AST rewriter ──────────────────────────────────────────────────────────────

struct SpawnRewriter;

impl VisitMut for SpawnRewriter {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // Recurse first so nested calls are handled.
        visit_mut::visit_expr_mut(self, expr);

        if let Expr::Call(ExprCall { func, .. }) = expr {
            if is_tokio_spawn(func) {
                // Replace the callee path in-place.
                *func = Box::new(parse_spawn_with_ctx());
            }
        }
    }
}

/// Returns true for `tokio::spawn` and bare `spawn` (when called as a free fn).
fn is_tokio_spawn(expr: &Expr) -> bool {
    match expr {
        Expr::Path(ExprPath { path, .. }) => is_tokio_spawn_path(path),
        _ => false,
    }
}

fn is_tokio_spawn_path(path: &Path) -> bool {
    let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    matches!(
        segs.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
        ["tokio", "spawn"] | ["tokio", "task", "spawn"]
    )
}

fn parse_spawn_with_ctx() -> Expr {
    syn::parse_quote! { ::replay_core::spawn_with_ctx }
}
