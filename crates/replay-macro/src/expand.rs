use proc_macro2::TokenStream;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, ReturnType};

pub fn expand_record_io(func: ItemFn) -> syn::Result<TokenStream> {
    // Only async functions are supported
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig.fn_token,
            "#[record_io] can only be applied to `async fn`",
        ));
    }

    let vis       = &func.vis;
    let sig       = &func.sig;
    let fn_name   = &func.sig.ident;
    let body      = &func.block;
    let inputs    = &func.sig.inputs;
    let _generics = &func.sig.generics; // preserved in sig, referenced via #sig
    let output    = &func.sig.output;

    // Return type as a token stream (defaults to `()`)
    let return_ty = match output {
        ReturnType::Default       => quote! { () },
        ReturnType::Type(_, ty)   => quote! { #ty },
    };

    // Collect non-self argument idents for serialization
    let arg_idents: Vec<_> = inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pat_type) => match &*pat_type.pat {
                Pat::Ident(pi) => Some(pi.ident.clone()),
                _              => None,
            },
            FnArg::Receiver(_) => None, // skip `self`
        })
        .collect();

    let arg_name_strs: Vec<String> = arg_idents.iter().map(|i| i.to_string()).collect();

    // Full path used as the stable fingerprint:
    //   module_path!() + "::" + fn_name  e.g. "my_crate::payments::compute_fee"
    let fn_name_str = fn_name.to_string();

    Ok(quote! {
        #vis #sig {
            // Build the stable fingerprint at call time
            let __fp = {
                let mut s = ::std::module_path!().to_string();
                s.push_str("::");
                s.push_str(#fn_name_str);
                s
            };

            // Allocate a slot from the ambient MockContext (None if context is off)
            let __slot = ::replay_core::next_interaction_slot(
                ::replay_core::CallType::Function,
                __fp.clone(),
            );

            match __slot.as_ref().map(|s| &s.mode) {
                // ── Replay: return the stored value without executing the body ──
                Some(::replay_core::ReplayMode::Replay) => {
                    let __slot = __slot.unwrap();
                    match ::replay_core::replay_fn_call(&__slot).await {
                        Some(__val) => {
                            ::serde_json::from_value::<#return_ty>(__val)
                                .unwrap_or_else(|e| panic!(
                                    "ditto #[record_io]: failed to deserialize stored \
                                     return value for `{}`: {e}",
                                    #fn_name_str
                                ))
                        }
                        // Cache miss — fall through to real execution
                        None => { #body }
                    }
                }

                // ── Record / Passthrough / Off: run the real body ─────────────
                _ => {
                    // Serialize arguments (graceful fallback to Debug repr)
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
                    let __result: #return_ty = { #body };

                    // Only write to the store when in Record mode
                    if let Some(__slot) = __slot {
                        if matches!(__slot.mode, ::replay_core::ReplayMode::Record) {
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
    })
}
