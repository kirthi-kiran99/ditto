use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ItemTrait;

use crate::expand::expand_trait_wrapper_method;

/// Expand `#[instrument_trait]` applied to a trait definition.
///
/// Re-emits the original trait unchanged and additionally generates:
///
/// ```text
/// pub struct ReplayMyTrait<T: MyTrait>(pub T);
/// impl<T: MyTrait> MyTrait for ReplayMyTrait<T> { ... }
/// ```
///
/// Each async method in the wrapper calls `self.0.method(args).await` inside
/// the standard record/replay body.  Sync methods delegate directly.
pub fn expand_instrument_trait(trait_def: ItemTrait) -> syn::Result<TokenStream> {
    let trait_name   = &trait_def.ident;
    let wrapper_name = format_ident!("Replay{}", trait_name);
    let vis          = &trait_def.vis;

    // Collect the trait's own generic parameters (lifetimes, types, consts).
    // For the common case of `trait Foo { ... }` this is empty.
    let trait_params = &trait_def.generics.params;
    let trait_where  = &trait_def.generics.where_clause;

    // Build ty_generics for referring to the trait: `MyTrait<A, B>` or just `MyTrait`.
    let trait_ty_args: TokenStream = if trait_params.is_empty() {
        quote! {}
    } else {
        // Collect only the ident/lifetime used as arguments (strip bounds).
        let args = trait_params.iter().map(|p| match p {
            syn::GenericParam::Type(t)     => { let i = &t.ident; quote! { #i } }
            syn::GenericParam::Lifetime(l) => { let l = &l.lifetime; quote! { #l } }
            syn::GenericParam::Const(c)    => { let i = &c.ident; quote! { #i } }
        });
        quote! { < #(#args),* > }
    };

    // Combined generics for impl: trait params + T.
    let combined_params: TokenStream = if trait_params.is_empty() {
        quote! { <T> }
    } else {
        quote! { <#trait_params, T> }
    };

    // Generate wrapper impl methods.
    let method_tokens: Vec<TokenStream> = trait_def
        .items
        .iter()
        .filter_map(|item| {
            if let syn::TraitItem::Fn(method) = item {
                Some(expand_trait_wrapper_method(method))
            } else {
                None
            }
        })
        .collect();

    // Associated type re-declarations.
    let assoc_types: Vec<TokenStream> = trait_def
        .items
        .iter()
        .filter_map(|item| {
            if let syn::TraitItem::Type(assoc) = item {
                let name = &assoc.ident;
                Some(quote! { type #name = T::#name; })
            } else {
                None
            }
        })
        .collect();

    // Associated const re-declarations.
    let assoc_consts: Vec<TokenStream> = trait_def
        .items
        .iter()
        .filter_map(|item| {
            if let syn::TraitItem::Const(c) = item {
                let name = &c.ident;
                let ty   = &c.ty;
                Some(quote! { const #name: #ty = T::#name; })
            } else {
                None
            }
        })
        .collect();

    Ok(quote! {
        // Re-emit the original trait definition unchanged.
        #trait_def

        // The newtype wrapper struct.
        #[allow(dead_code)]
        #vis struct #wrapper_name #combined_params(pub T)
        where
            T: #trait_name #trait_ty_args,
            #trait_where;

        // The instrumented impl.
        impl #combined_params #trait_name #trait_ty_args
            for #wrapper_name #combined_params
        where
            T: #trait_name #trait_ty_args,
            #trait_where
        {
            #(#assoc_types)*
            #(#assoc_consts)*
            #(#method_tokens)*
        }
    })
}
