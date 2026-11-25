//! Generate lightweight mock Soroban contracts from trait definitions.
//!
//! Testing a contract that calls another contract normally means writing a
//! second contract that implements the dependency's whole interface. Apply
//! [`macro@mock_contract`] to a trait describing that interface instead, and
//! the macro generates:
//!
//! - `Mock{Trait}Contract` — a real `#[contract]` exporting every trait
//!   function, which any contract can call through the dependency's client;
//! - `Mock{Trait}` — a handle that registers the contract, programs return
//!   values per function, records every call and verifies expectations;
//! - `Mock{Trait}Call` — an enum with one variant per function, so the call
//!   history can be matched on and compared;
//! - `Mock{Trait}{Function}Expectation` — the builder returned by
//!   `expect_{function}()`.
//!
//! ```ignore
//! use soroban_sdk::{Address, Env};
//! use soroban_testkit_mock::mock_contract;
//!
//! #[mock_contract]
//! pub trait TokenInterface {
//!     fn transfer(from: Address, to: Address, amount: i128);
//!     fn balance_of(account: Address) -> i128;
//! }
//!
//! let env = Env::default();
//! let mock = MockTokenInterface::new(&env);
//! mock.expect_balance_of().returning(|_| 1000);
//! mock.expect_transfer().times(1);
//!
//! // ...hand `mock.address()` to the contract under test and exercise it...
//!
//! assert_eq!(mock.balance_of_calls().len(), 1);
//! mock.verify();
//! ```
//!
//! Functions may declare an `env: Env` first parameter, the shape
//! `#[contractclient]` expects. When every function does, the generated
//! contract implements the trait itself (`impl Trait for Mock{Trait}Contract`),
//! so the compiler checks the mock against the interface.
//!
//! The generated code uses `std` (thread-local state and closures), so use it
//! from integration tests or from a `#[cfg(test)]` module.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, spanned::Spanned, FnArg, Ident, ItemTrait, Pat, ReturnType, TraitItem,
    TraitItemFn, Type,
};

/// Generate a mock contract from a trait definition. See the
/// [crate documentation](crate) for what is generated.
#[proc_macro_attribute]
pub fn mock_contract(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "#[mock_contract] takes no arguments")
            .to_compile_error()
            .into();
    }
    let input = parse_macro_input!(item as ItemTrait);
    match expand(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

struct Method {
    name: Ident,
    variant: Ident,
    env_arg: Option<Ident>,
    arg_names: Vec<Ident>,
    arg_types: Vec<Type>,
    output: Option<Type>,
    expectation: Ident,
    expectation_field: Ident,
    calls_fn: Ident,
    expect_fn: Ident,
}

fn camel_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

fn is_env(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Env"),
        _ => false,
    }
}

fn parse_method(mock: &Ident, item: &TraitItemFn) -> syn::Result<Method> {
    let sig = &item.sig;
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            sig.generics.span(),
            "#[mock_contract] functions cannot be generic: contract functions have concrete types",
        ));
    }
    if sig.asyncness.is_some() {
        return Err(syn::Error::new(
            sig.span(),
            "#[mock_contract] functions cannot be async",
        ));
    }
    let mut env_arg = None;
    let mut arg_names = Vec::new();
    let mut arg_types = Vec::new();
    for (index, input) in sig.inputs.iter().enumerate() {
        let typed = match input {
            FnArg::Receiver(receiver) => {
                return Err(syn::Error::new(
                    receiver.span(),
                    "#[mock_contract] traits describe contract functions, which take no receiver; remove `self`",
                ))
            }
            FnArg::Typed(typed) => typed,
        };
        let name = match &*typed.pat {
            Pat::Ident(ident) => ident.ident.clone(),
            _ => format_ident!("arg{}", index),
        };
        if index == 0 && is_env(&typed.ty) {
            env_arg = Some(name);
            continue;
        }
        arg_names.push(name);
        arg_types.push((*typed.ty).clone());
    }
    let output = match &sig.output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => match &**ty {
            Type::Tuple(tuple) if tuple.elems.is_empty() => None,
            ty => Some(ty.clone()),
        },
    };
    let name = sig.ident.clone();
    let camel = camel_case(&name.to_string());
    Ok(Method {
        variant: format_ident!("{}", camel),
        expectation: format_ident!("{}{}Expectation", mock, camel),
        expectation_field: format_ident!("expect_{}", name),
        calls_fn: format_ident!("{}_calls", name),
        expect_fn: format_ident!("expect_{}", name),
        env_arg,
        arg_names,
        arg_types,
        output,
        name,
    })
}

fn expand(input: &ItemTrait) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "#[mock_contract] traits cannot be generic",
        ));
    }
    let trait_name = &input.ident;
    let vis = &input.vis;
    let mock = format_ident!("Mock{}", trait_name);
    let contract = format_ident!("Mock{}Contract", trait_name);
    let call_enum = format_ident!("Mock{}Call", trait_name);
    let state = format_ident!("Mock{}State", trait_name);
    let module = format_ident!("__soroban_testkit_mock_{}", trait_name);

    let methods = input
        .items
        .iter()
        .filter_map(|item| match item {
            TraitItem::Fn(item) => Some(parse_method(&mock, item)),
            _ => None,
        })
        .collect::<syn::Result<Vec<_>>>()?;
    for item in &input.items {
        if !matches!(item, TraitItem::Fn(_)) {
            return Err(syn::Error::new(
                item.span(),
                "#[mock_contract] traits may only contain functions",
            ));
        }
    }
    if methods.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "#[mock_contract] needs at least one function to mock",
        ));
    }
    let implements_trait = methods.iter().all(|method| method.env_arg.is_some());
    let trait_name_str = trait_name.to_string();

    // ── call enum ─────────────────────────────────────────────────────────
    let call_variants = methods.iter().map(|method| {
        let variant = &method.variant;
        let names = &method.arg_names;
        let types = &method.arg_types;
        let doc = format!("A call to `{}`.", method.name);
        quote! {
            #[doc = #doc]
            #variant { #( #names: #types, )* }
        }
    });
    let call_name_arms = methods.iter().map(|method| {
        let variant = &method.variant;
        let name = method.name.to_string();
        quote! { #call_enum::#variant { .. } => #name }
    });

    // ── per-function expectation storage and builders ─────────────────────
    let expectation_items = methods.iter().map(|method| {
        let expectation = &method.expectation;
        let field = &method.expectation_field;
        let types = &method.arg_types;
        let names = &method.arg_names;
        let output = method
            .output
            .as_ref()
            .map(|ty| quote!(#ty))
            .unwrap_or_else(|| quote!(()));
        let entry = format_ident!("{}Entry", expectation);
        let ignored: Vec<Ident> = names
            .iter()
            .map(|name| format_ident!("_{}", name))
            .collect();
        let actual: Vec<Ident> = names
            .iter()
            .map(|name| format_ident!("actual_{}", name))
            .collect();
        let doc = format!(
            "Programs the mock's `{}` function. Returned by `expect_{}()`.",
            method.name, method.name
        );
        quote! {
            #[doc(hidden)]
            pub struct #entry {
                matcher: ::core::option::Option<std::rc::Rc<dyn Fn(#( &#types ),*) -> bool>>,
                returning: ::core::option::Option<std::rc::Rc<dyn Fn(#( &#types ),*) -> #output>>,
                times: ::core::option::Option<usize>,
                hits: usize,
            }

            #[doc = #doc]
            ///
            /// Expectations are tried in the order they were created; the
            /// first whose argument matcher accepts a call and whose `times`
            /// limit is not yet reached handles it.
            pub struct #expectation<'a> {
                mock: &'a #mock,
                index: usize,
            }

            impl<'a> #expectation<'a> {
                fn update(self, f: impl FnOnce(&mut #entry)) -> Self {
                    let id = self.mock.id;
                    let index = self.index;
                    REGISTRY.with(|registry| {
                        let mut registry = registry.borrow_mut();
                        let state = registry.get_mut(&id).expect("the mock is alive");
                        f(&mut state.#field[index]);
                    });
                    self
                }

                /// Compute the return value from the call's arguments.
                pub fn returning<F>(self, f: F) -> Self
                where
                    F: Fn(#( &#types ),*) -> #output + 'static,
                {
                    self.update(move |entry| entry.returning = Some(std::rc::Rc::new(f)))
                }

                /// Return a clone of `value` on every matching call.
                pub fn returns(self, value: #output) -> Self {
                    self.returning(move |#( #ignored: &#types ),*| value.clone())
                }

                /// Only handle calls whose arguments equal these.
                pub fn with(self, #( #names: #types ),*) -> Self {
                    self.withf(move |#( #actual ),*| true #( && *#actual == #names )* )
                }

                /// Only handle calls whose arguments satisfy `predicate`.
                pub fn withf<F>(self, predicate: F) -> Self
                where
                    F: Fn(#( &#types ),*) -> bool + 'static,
                {
                    self.update(move |entry| entry.matcher = Some(std::rc::Rc::new(predicate)))
                }

                /// Expect exactly `n` matching calls; checked by `verify()`.
                pub fn times(self, n: usize) -> Self {
                    self.update(move |entry| entry.times = Some(n))
                }

                /// Expect exactly one matching call.
                pub fn once(self) -> Self {
                    self.times(1)
                }

                /// Expect no matching call.
                pub fn never(self) -> Self {
                    self.times(0)
                }
            }
        }
    });

    let state_fields = methods.iter().map(|method| {
        let field = &method.expectation_field;
        let entry = format_ident!("{}Entry", method.expectation);
        quote! { #field: std::vec::Vec<#entry>, }
    });
    let state_defaults = methods.iter().map(|method| {
        let field = &method.expectation_field;
        quote! { #field: std::vec::Vec::new(), }
    });
    let state_clears = methods.iter().map(|method| {
        let field = &method.expectation_field;
        quote! { state.#field.clear(); }
    });
    let verify_checks = methods.iter().map(|method| {
        let field = &method.expectation_field;
        let name = method.name.to_string();
        quote! {
            for (index, entry) in state.#field.iter().enumerate() {
                if let Some(expected) = entry.times {
                    if entry.hits != expected {
                        failures.push(std::format!(
                            "{}::{} expectation #{}: expected {} call(s), got {}",
                            #trait_name_str, #name, index, expected, entry.hits
                        ));
                    }
                }
            }
        }
    });

    // ── handle methods ────────────────────────────────────────────────────
    let handle_methods = methods.iter().map(|method| {
        let variant = &method.variant;
        let names = &method.arg_names;
        let types = &method.arg_types;
        let calls_fn = &method.calls_fn;
        let expect_fn = &method.expect_fn;
        let expectation = &method.expectation;
        let entry = format_ident!("{}Entry", expectation);
        let field = &method.expectation_field;
        let calls_doc = format!(
            "Arguments of every recorded call to `{}`, oldest first.",
            method.name
        );
        let expect_doc = format!("Add an expectation for `{}`.", method.name);
        quote! {
            #[doc = #calls_doc]
            pub fn #calls_fn(&self) -> std::vec::Vec<( #( #types, )* )> {
                self.calls()
                    .into_iter()
                    .filter_map(|call| match call {
                        #call_enum::#variant { #( #names, )* } => Some(( #( #names, )* )),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect()
            }

            #[doc = #expect_doc]
            pub fn #expect_fn(&self) -> #expectation<'_> {
                let index = REGISTRY.with(|registry| {
                    let mut registry = registry.borrow_mut();
                    let state = registry.get_mut(&self.id).expect("the mock is alive");
                    state.#field.push(#entry {
                        matcher: None,
                        returning: None,
                        times: None,
                        hits: 0,
                    });
                    state.#field.len() - 1
                });
                #expectation { mock: self, index }
            }
        }
    });

    // ── contract functions ────────────────────────────────────────────────
    let contract_fns = methods.iter().map(|method| {
        let name = &method.name;
        let name_str = name.to_string();
        let variant = &method.variant;
        let field = &method.expectation_field;
        let names = &method.arg_names;
        let types = &method.arg_types;
        let env = method
            .env_arg
            .clone()
            .unwrap_or_else(|| format_ident!("env"));
        let output = method.output.as_ref().map(|ty| quote!(-> #ty));
        let finish = if method.output.is_some() {
            quote! {
                match behaviour {
                    Some(Some(f)) => f(#( &#names ),*),
                    Some(None) => panic!(
                        "mock {}::{} matched an expectation with no return value; call .returning() or .returns()",
                        #trait_name_str, #name_str
                    ),
                    None => panic!(
                        "unexpected call to mock {}::{}: no expectation matches; add one with expect_{}()",
                        #trait_name_str, #name_str, #name_str
                    ),
                }
            }
        } else {
            quote! {
                if let Some(Some(f)) = behaviour {
                    f(#( &#names ),*)
                }
            }
        };
        let visibility = if implements_trait { quote!() } else { quote!(pub) };
        quote! {
            #visibility fn #name(#env: soroban_sdk::Env, #( #names: #types ),*) #output {
                let id: u64 = #env
                    .storage()
                    .instance()
                    .get(&MOCK_ID_KEY)
                    .expect("mock contract was registered by its handle");
                let behaviour = REGISTRY.with(|registry| {
                    let mut registry = registry.borrow_mut();
                    let state = registry
                        .get_mut(&id)
                        .expect("mock handle dropped while its contract was still called");
                    state.calls.push(#call_enum::#variant { #( #names: #names.clone(), )* });
                    let entries = &mut state.#field;
                    let accepts = |matcher: &::core::option::Option<std::rc::Rc<dyn Fn(#( &#types ),*) -> bool>>| {
                        matcher.as_ref().map_or(true, |m| m(#( &#names ),*))
                    };
                    let chosen = entries
                        .iter()
                        .position(|entry| accepts(&entry.matcher) && entry.times.map_or(true, |t| entry.hits < t))
                        .or_else(|| entries.iter().rposition(|entry| accepts(&entry.matcher)));
                    chosen.map(|index| {
                        entries[index].hits += 1;
                        entries[index].returning.clone()
                    })
                });
                #finish
            }
        }
    });

    let contract_impl = if implements_trait {
        quote! {
            #[soroban_sdk::contractimpl]
            impl super::#trait_name for #contract {
                #( #contract_fns )*
            }
        }
    } else {
        quote! {
            #[soroban_sdk::contractimpl]
            impl #contract {
                #( #contract_fns )*
            }
        }
    };

    let mock_doc = format!(
        "Handle to a registered mock of `{trait_name}`. Program it with the \
         `expect_*` methods, read the call history with `calls()` and the \
         `*_calls()` methods, and check expectations with `verify()`."
    );
    let contract_doc = format!("Mock contract exporting every function of `{trait_name}`.");
    let call_doc = format!("One recorded call to the `{trait_name}` mock.");

    Ok(quote! {
        #input

        #[doc(hidden)]
        #[allow(non_snake_case, dead_code, unused_imports, clippy::all, clippy::pedantic)]
        mod #module {
            extern crate std;
            use super::*;
            use std::prelude::rust_2021::*;

            const MOCK_ID_KEY: soroban_sdk::Symbol = soroban_sdk::symbol_short!("mock_id");

            static NEXT_ID: ::core::sync::atomic::AtomicU64 = ::core::sync::atomic::AtomicU64::new(1);

            std::thread_local! {
                static REGISTRY: ::core::cell::RefCell<std::collections::HashMap<u64, #state>> =
                    ::core::cell::RefCell::new(std::collections::HashMap::new());
            }

            #[doc = #call_doc]
            #[derive(Clone, Debug, PartialEq)]
            pub enum #call_enum {
                #( #call_variants, )*
            }

            impl #call_enum {
                /// The name of the function this call invoked.
                pub fn function(&self) -> &'static str {
                    match self {
                        #( #call_name_arms, )*
                    }
                }
            }

            #[doc(hidden)]
            pub struct #state {
                calls: std::vec::Vec<#call_enum>,
                #( #state_fields )*
            }

            #( #expectation_items )*

            #[doc = #contract_doc]
            #[soroban_sdk::contract]
            pub struct #contract;

            #[soroban_sdk::contractimpl]
            impl #contract {
                pub fn __constructor(env: soroban_sdk::Env, mock_id: u64) {
                    env.storage().instance().set(&MOCK_ID_KEY, &mock_id);
                }
            }

            #contract_impl

            #[doc = #mock_doc]
            pub struct #mock {
                env: soroban_sdk::Env,
                id: u64,
                address: soroban_sdk::Address,
            }

            impl #mock {
                /// Register a fresh mock contract in `env`.
                pub fn new(env: &soroban_sdk::Env) -> Self {
                    let id = NEXT_ID.fetch_add(1, ::core::sync::atomic::Ordering::Relaxed);
                    REGISTRY.with(|registry| {
                        registry.borrow_mut().insert(id, #state {
                            calls: std::vec::Vec::new(),
                            #( #state_defaults )*
                        });
                    });
                    let address = env.register(#contract, (id,));
                    Self { env: env.clone(), id, address }
                }

                /// The mock contract's address, to pass to the contract under test.
                pub fn address(&self) -> soroban_sdk::Address {
                    self.address.clone()
                }

                /// The environment the mock is registered in.
                pub fn env(&self) -> &soroban_sdk::Env {
                    &self.env
                }

                /// Every recorded call, oldest first.
                pub fn calls(&self) -> std::vec::Vec<#call_enum> {
                    REGISTRY.with(|registry| {
                        registry
                            .borrow()
                            .get(&self.id)
                            .map(|state| state.calls.clone())
                            .unwrap_or_default()
                    })
                }

                /// How many calls the mock has received.
                pub fn call_count(&self) -> usize {
                    REGISTRY.with(|registry| {
                        registry.borrow().get(&self.id).map_or(0, |state| state.calls.len())
                    })
                }

                /// Panic with every unmet `times` expectation.
                pub fn verify(&self) {
                    let failures = REGISTRY.with(|registry| {
                        let registry = registry.borrow();
                        let mut failures: std::vec::Vec<std::string::String> = std::vec::Vec::new();
                        if let Some(state) = registry.get(&self.id) {
                            #( #verify_checks )*
                        }
                        failures
                    });
                    if !failures.is_empty() {
                        panic!("mock {} expectations not met:\n  {}", #trait_name_str, failures.join("\n  "));
                    }
                }

                /// `verify()`, then forget every expectation and recorded call.
                pub fn checkpoint(&self) {
                    self.verify();
                    self.reset();
                }

                /// Forget every expectation and recorded call without verifying.
                pub fn reset(&self) {
                    REGISTRY.with(|registry| {
                        if let Some(state) = registry.borrow_mut().get_mut(&self.id) {
                            state.calls.clear();
                            #( #state_clears )*
                        }
                    });
                }

                #( #handle_methods )*
            }

            impl Drop for #mock {
                fn drop(&mut self) {
                    let _ = REGISTRY.try_with(|registry| {
                        if let Ok(mut registry) = registry.try_borrow_mut() {
                            registry.remove(&self.id);
                        }
                    });
                }
            }
        }

        #[allow(unused_imports)]
        #vis use #module::*;
    })
}
