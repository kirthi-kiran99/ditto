use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ImplItemFn, Pat, ReturnType, Signature, Visibility};

// ── shared body generator ─────────────────────────────────────────────────────

/// Generate the record/replay function body for any async fn.
///
/// `body_tokens` is the real execution expression — either `{ original_block }`
/// for `#[record_io]` / `#[instrument]`, or `self.0.method(args).await` for
/// `#[instrument_trait]` wrapper methods.
pub(crate) fn gen_record_replay_body(
    vis:            &Visibility,
    sig:            &Signature,
    fn_name_str:    &str,
    arg_idents:     &[syn::Ident],
    arg_name_strs:  &[String],
    return_ty:      &TokenStream,
    body_tokens:    TokenStream,
) -> TokenStream {
    quote! {
        #vis #sig {
            let __fp = {
                let mut s = ::std::module_path!().to_string();
                s.push_str("::");
                s.push_str(#fn_name_str);
                s
            };

            let __slot = ::replay_core::next_interaction_slot(
                ::replay_core::CallType::Function,
                __fp.clone(),
            );

            match __slot.as_ref().map(|s| &s.mode) {
                Some(::replay_core::ReplayMode::Replay) => {
                    let __slot = __slot.unwrap();
                    match ::replay_core::replay_fn_call(&__slot).await {
                        Some(__val) => {
                            ::serde_json::from_value::<#return_ty>(__val)
                                .unwrap_or_else(|e| panic!(
                                    "ditto: failed to deserialize stored return value \
                                     for `{}`: {e}",
                                    #fn_name_str
                                ))
                        }
                        None => { #body_tokens }
                    }
                }

                _ => {
                    let __request: ::serde_json::Value = {
                        let mut __m = ::serde_json::Map::new();
                        #(
                            {
                                let __v = ::serde_json::to_value(&#arg_idents)
                                    .unwrap_or_else(|_| {
                                        ::serde_json::Value::String(
                                            format!("{:?}", &#arg_idents)
                                        )
                                    });
                                __m.insert(#arg_name_strs.to_string(), __v);
                            }
                        )*
                        ::serde_json::Value::Object(__m)
                    };

                    let __start = ::std::time::Instant::now();
                    let __result: #return_ty = { #body_tokens };

                    if let Some(__slot) = __slot {
                        if matches!(
                            __slot.mode,
                            ::replay_core::ReplayMode::Record | ::replay_core::ReplayMode::Shadow
                        ) {
                            let __response = ::serde_json::to_value(&__result)
                                .unwrap_or(::serde_json::Value::Null);
                            let __ms = __start.elapsed().as_millis() as u64;
                            ::replay_core::record_fn_call(
                                __slot, __request, __response, __ms,
                            )
                            .await;
                        }
                    }

                    __result
                }
            }
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

pub(crate) fn extract_args(inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>)
    -> (Vec<syn::Ident>, Vec<String>)
{
    let idents: Vec<_> = inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pt) => match &*pt.pat {
                Pat::Ident(pi) => Some(pi.ident.clone()),
                _              => None,
            },
            FnArg::Receiver(_) => None,
        })
        .collect();
    let strs = idents.iter().map(|i| i.to_string()).collect();
    (idents, strs)
}

pub(crate) fn return_type(output: &ReturnType) -> TokenStream {
    match output {
        ReturnType::Default     => quote! { () },
        ReturnType::Type(_, ty) => quote! { #ty },
    }
}

// ── #[record_io] ──────────────────────────────────────────────────────────────

pub fn expand_record_io(func: syn::ItemFn) -> syn::Result<TokenStream> {
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig.fn_token,
            "#[record_io] can only be applied to `async fn`",
        ));
    }

    let vis  = &func.vis;
    let sig  = &func.sig;
    let body = &func.block;
    let fn_name_str = sig.ident.to_string();
    let (arg_idents, arg_name_strs) = extract_args(&sig.inputs);
    let return_ty = return_type(&sig.output);

    Ok(gen_record_replay_body(
        vis, sig, &fn_name_str,
        &arg_idents, &arg_name_strs, &return_ty,
        quote! { #body },
    ))
}

// ── expand a single async impl method ────────────────────────────────────────

/// Instrument one async impl method.  Sync methods pass through unchanged.
pub(crate) fn expand_impl_method(method: &ImplItemFn) -> syn::Result<TokenStream> {
    if method.sig.asyncness.is_none() {
        return Ok(quote! { #method });
    }

    let vis  = &method.vis;
    let sig  = &method.sig;
    let body = &method.block;
    let fn_name_str = sig.ident.to_string();
    let (arg_idents, arg_name_strs) = extract_args(&sig.inputs);
    let return_ty = return_type(&sig.output);

    Ok(gen_record_replay_body(
        vis, sig, &fn_name_str,
        &arg_idents, &arg_name_strs, &return_ty,
        quote! { #body },
    ))
}

// ── expand a trait method as a delegation wrapper ─────────────────────────────

/// Generate a wrapper method that delegates to `self.0.method(args)`.
/// Used by `#[instrument_trait]` to generate the impl for the wrapper struct.
pub(crate) fn expand_trait_wrapper_method(
    method: &syn::TraitItemFn,
) -> TokenStream {
    let sig = &method.sig;
    let name = &sig.ident;
    let fn_name_str = name.to_string();
    let (arg_idents, arg_name_strs) = extract_args(&sig.inputs);
    let return_ty = return_type(&sig.output);

    if sig.asyncness.is_none() {
        // Sync methods: simple delegation, no instrumentation
        return quote! {
            #sig {
                self.0.#name(#(#arg_idents),*)
            }
        };
    }

    // For async methods, use gen_record_replay_body with a delegation expression
    let delegate = quote! { self.0.#name(#(#arg_idents),*).await };

    // gen_record_replay_body expects a Visibility; trait impls have no explicit vis
    let no_vis: Visibility = syn::parse_quote! {};

    gen_record_replay_body(
        &no_vis, sig, &fn_name_str,
        &arg_idents, &arg_name_strs, &return_ty,
        delegate,
    )
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn record_io_rejects_sync_fn() {
        let func: syn::ItemFn = parse_quote! {
            fn sync_fn() -> u64 { 42 }
        };
        let result = expand_record_io(func);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("async fn"),
            "error message must mention `async fn`"
        );
    }

    #[test]
    fn record_io_accepts_async_fn() {
        let func: syn::ItemFn = parse_quote! {
            async fn my_fn(x: u64) -> u64 { x }
        };
        assert!(expand_record_io(func).is_ok());
    }

    #[test]
    fn expand_impl_method_passes_through_sync() {
        let method: ImplItemFn = parse_quote! {
            fn sync_method(&self) -> &str { "sync" }
        };
        let ts = expand_impl_method(&method).unwrap();
        // The output should be identical to the original (no instrumentation)
        let reparsed: ImplItemFn = syn::parse2(ts).unwrap();
        assert_eq!(
            reparsed.sig.ident.to_string(),
            "sync_method"
        );
        assert!(reparsed.sig.asyncness.is_none());
    }

    #[test]
    fn expand_impl_method_instruments_async() {
        let method: ImplItemFn = parse_quote! {
            async fn compute(&self, x: u64) -> u64 { x * 2 }
        };
        let ts = expand_impl_method(&method).unwrap();
        let ts_str = ts.to_string();
        // Generated code should reference replay_core
        assert!(ts_str.contains("replay_core"), "generated code must reference replay_core");
        assert!(ts_str.contains("next_interaction_slot"), "must allocate an interaction slot");
    }
}
