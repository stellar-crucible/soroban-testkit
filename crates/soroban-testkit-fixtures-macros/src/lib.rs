//! Procedural macros for [`soroban-testkit-fixtures`]' declarative mock
//! contracts.
//!
//! See [`mock_contract`] in that crate for the user-facing documentation.

use proc_macro::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{parse_macro_input, FnArg, ImplItem, ItemImpl, Pat, Type};

/// Turns the functions of a `#[contractimpl]` block into dispatchers for the
/// expectations registered by `MockContractBuilder`.
///
/// Apply it directly above `#[contractimpl]`:
///
/// ```ignore
/// #[contract]
/// pub struct RateOracle;
///
/// #[mock_contract]
/// #[contractimpl]
/// impl RateOracle {
///     pub fn get_rate(_env: Env, asset: Address) -> i128 { unimplemented!() }
/// }
/// ```
///
/// The attribute rewrites every function body, so the bodies themselves are
/// never compiled or run; `unimplemented!()` is only a placeholder. It also
/// generates the `MockContractType` implementation the builder registers with.
#[proc_macro_attribute]
pub fn mock_contract(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut block = parse_macro_input!(item as ItemImpl);
    let self_ty = block.self_ty.clone();
    let mut errors = Vec::new();

    for item in &mut block.items {
        let ImplItem::Fn(function) = item else {
            continue;
        };
        let name = function.sig.ident.to_string();

        let Some(first) = function.sig.inputs.first() else {
            errors.push(quote_spanned! { function.sig.span() =>
                compile_error!("mock_contract: every mocked function needs an `env: Env` first parameter");
            });
            continue;
        };
        let env_ident = match first {
            FnArg::Typed(pat) => match (&*pat.pat, &*pat.ty) {
                (Pat::Ident(ident), Type::Path(path)) if path.path.is_ident("Env") => {
                    ident.ident.clone()
                }
                (Pat::Ident(ident), Type::Reference(reference)) if matches!(&*reference.elem, Type::Path(path) if path.path.is_ident("Env")) => {
                    ident.ident.clone()
                }
                _ => {
                    errors.push(quote_spanned! { pat.span() =>
                        compile_error!("mock_contract: the first parameter must be `env: Env`");
                    });
                    continue;
                }
            },
            FnArg::Receiver(receiver) => {
                errors.push(quote_spanned! { receiver.span() =>
                    compile_error!("mock_contract: `self` parameters are not supported");
                });
                continue;
            }
        };
        let env_value = quote!(&#env_ident);

        let mut conversions = Vec::new();
        for argument in function.sig.inputs.iter().skip(1) {
            let FnArg::Typed(pat) = argument else {
                continue;
            };
            let Pat::Ident(ident) = &*pat.pat else {
                errors.push(quote_spanned! { pat.span() =>
                    compile_error!("mock_contract: parameters must be plain identifiers");
                });
                continue;
            };
            let ident = &ident.ident;
            conversions.push(quote! {
                soroban_sdk::IntoVal::<soroban_sdk::Env, soroban_sdk::Val>::into_val(
                    &#ident, &#env_ident,
                )
            });
        }

        function.block = syn::parse_quote!({
            let args: &[soroban_sdk::Val] = &[#(#conversions),*];
            ::soroban_testkit_fixtures::mock::dispatch(#env_value, #name, args)
        });
    }

    if !errors.is_empty() {
        let errors = quote! { #(#errors)* };
        return quote! {
            #block
            #errors
        }
        .into();
    }

    quote! {
        #block

        impl ::soroban_testkit_fixtures::mock::MockContractType for #self_ty {
            fn register_mock(env: &soroban_sdk::Env) -> soroban_sdk::Address {
                env.register(#self_ty, ())
            }
        }
    }
    .into()
}
