use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemImpl;

use crate::expand::expand_impl_method;

/// Expand `#[instrument]` applied to an `impl` block.
///
/// Every `async fn` inside the block is wrapped with record/replay logic.
/// Sync methods pass through unchanged.
pub fn expand_instrument(mut impl_block: ItemImpl) -> syn::Result<TokenStream> {
    let mut instrumented_items = Vec::new();

    for item in &impl_block.items {
        match item {
            syn::ImplItem::Fn(method) => {
                let ts = expand_impl_method(method)?;
                instrumented_items.push(ts);
            }
            other => {
                instrumented_items.push(quote! { #other });
            }
        }
    }

    // Reconstruct the impl block with instrumented methods.
    let attrs      = &impl_block.attrs;
    let defaultness = &impl_block.defaultness;
    let unsafety   = &impl_block.unsafety;
    let impl_token = &impl_block.impl_token;
    let generics   = &impl_block.generics;
    let trait_     = impl_block.trait_.as_ref().map(|(bang, path, for_)| {
        quote! { #bang #path #for_ }
    });
    let self_ty    = &impl_block.self_ty;

    // Clear items so we can replace them.
    impl_block.items.clear();

    Ok(quote! {
        #(#attrs)*
        #defaultness #unsafety #impl_token #generics #trait_ #self_ty {
            #(#instrumented_items)*
        }
    })
}
