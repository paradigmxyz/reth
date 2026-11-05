// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.
use crate::attributes::ParamKind;
use crate::helpers::generate_where_clause;
use crate::rpc_macro::{RpcDescription, RpcFnArg, RpcMethod, RpcSubscription};
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{AngleBracketedGenericArguments, FnArg, Ident, Pat, PatIdent, PatType, PathArguments, TypeParam};

impl RpcDescription {
	pub(super) fn render_client(&self) -> Result<TokenStream2, syn::Error> {
		let jsonrpsee = self.jsonrpsee_client_path.as_ref().unwrap();
		let sub_tys: Vec<syn::Type> = self.subscriptions.clone().into_iter().map(|s| s.item).collect();

		let trait_name = quote::format_ident!("{}Client", &self.trait_def.ident);
		let where_clause = generate_where_clause(&self.trait_def, &sub_tys, true, self.client_bounds.as_ref());
		let type_idents = self.trait_def.generics.type_params().collect::<Vec<&TypeParam>>();
		let (impl_generics, type_generics, _) = self.trait_def.generics.split_for_impl();

		let super_trait = if self.subscriptions.is_empty() {
			quote! { #jsonrpsee::core::client::ClientT }
		} else {
			quote! { #jsonrpsee::core::client::SubscriptionClientT }
		};

		let method_impls =
			self.methods.iter().map(|method| self.render_method(method)).collect::<Result<Vec<_>, _>>()?;
		let sub_impls = self.subscriptions.iter().map(|sub| self.render_sub(sub)).collect::<Result<Vec<_>, _>>()?;

		// Doc-comment to be associated with the client.
		let doc_comment = format!("Client implementation for the `{}` RPC API.", &self.trait_def.ident);
		let trait_impl = quote! {
			#[doc = #doc_comment]
			pub trait #trait_name #impl_generics: #super_trait where #(#where_clause,)* {
				#(#method_impls)*
				#(#sub_impls)*
			}

			impl<TypeJsonRpseeInternal #(,#type_idents)*> #trait_name #type_generics for TypeJsonRpseeInternal where TypeJsonRpseeInternal: #super_trait #(,#where_clause)* {}
		};

		Ok(trait_impl)
	}

	/// Verify and rewrite the return type (for methods).
	fn return_result_type(&self, mut ty: syn::Type) -> TokenStream2 {
		// We expect a valid type path.
		let syn::Type::Path(ref mut type_path) = ty else {
			return quote_spanned!(ty.span() => compile_error!("Expecting something like 'Result<Foo, Err>' here. (1)"));
		};

		// The path (eg std::result::Result) should have a final segment like 'Result'.
		let Some(type_name) = type_path.path.segments.last_mut() else {
			return quote_spanned!(ty.span() => compile_error!("Expecting this path to end in something like 'Result<Foo, Err>'"));
		};

		// Get the generic args eg the <T, E> in Result<T, E>.
		let PathArguments::AngleBracketed(AngleBracketedGenericArguments { args, .. }) = &mut type_name.arguments
		else {
			return quote_spanned!(ty.span() => compile_error!("Expecting something like 'Result<Foo, Err>' here, but got no generic args (eg no '<Foo,Err>')."));
		};

		if type_name.ident == "Result" {
			// Result<T, E> should have 2 generic args.
			if args.len() != 2 {
				return quote_spanned!(args.span() => compile_error!("Result must be have two arguments"));
			}

			// Force the last argument to be `jsonrpsee::core::ClientError`:
			let error_arg = args.last_mut().unwrap();
			*error_arg =
				syn::GenericArgument::Type(syn::Type::Verbatim(self.jrps_client_item(quote! { core::client::Error })));

			quote!(#ty)
		} else if type_name.ident == "RpcResult" {
			// RpcResult<T> (an alias we export) should have 1 generic arg.
			if args.len() != 1 {
				return quote_spanned!(args.span() => compile_error!("RpcResult must have one argument"));
			}

			// The type alias `RpcResult<T>` is modified to `Result<T, Error>`.
			let ret_ty = args.last_mut().unwrap();
			let err_ty = self.jrps_client_item(quote! { core::client::Error });

			quote! { core::result::Result<#ret_ty, #err_ty> }
		} else if type_name.ident == "ResponsePayload" {
			// ResponsePayload<'a, T>
			if args.len() != 2 {
				return quote_spanned!(args.span() => compile_error!("ResponsePayload must have exactly two arguments"));
			}

			// The type alias `RpcResult<T>` is modified to `Result<T, Error>`.
			let ret_ty = args.last_mut().unwrap();
			let err_ty = self.jrps_client_item(quote! { core::client::Error });

			quote! { core::result::Result<#ret_ty, #err_ty> }
		} else {
			// Any other type name isn't allowed.
			quote_spanned!(type_name.span() => compile_error!("The return type must be Result<T, Error>, RpcResult<T> or ResponsePayload<'static, T>"))
		}
	}

	fn render_method(&self, method: &RpcMethod) -> Result<TokenStream2, syn::Error> {
		// `jsonrpsee::Error`
		let jrps_error = self.jrps_client_item(quote! { core::client::Error });
		// Rust method to invoke (e.g. `self.<foo>(...)`).
		let rust_method_name = &method.signature.sig.ident;
		// List of inputs to put into `Params` (e.g. `self.foo(<12, "baz">)`).
		// Includes `&self` receiver.
		let rust_method_params = &method.signature.sig.inputs;
		// Name of the RPC method (e.g. `foo_makeSpam`).
		let rpc_method_name = self.rpc_identifier(&method.name);

		// Called method is either `request` or `notification`.
		// `returns` represent the return type of the *rust method* (`Result<T, jsonrpsee::core::ClientError>`).
		let (called_method, returns) = if let Some(returns) = &method.returns {
			let called_method = quote::format_ident!("request");
			let returns = self.return_result_type(returns.clone());
			let returns = quote! { impl core::future::Future<Output = #returns> + Send };

			(called_method, returns)
		} else {
			let called_method = quote::format_ident!("notification");
			let returns = quote! { impl core::future::Future<Output = Result<(), #jrps_error>> + Send };

			(called_method, returns)
		};

		// Encoded parameters for the request.
		let parameter_builder = self.encode_params(&method.params, &method.param_kind, &method.signature);
		// Doc-comment to be associated with the method.
		let docs = &method.docs;
		// Mark the method as deprecated, if previously declared as so.
		let deprecated = &method.deprecated;

		let method = quote! {
			#docs
			#deprecated
			#[allow(non_snake_case)]
			#[allow(clippy::used_underscore_binding)]
			fn #rust_method_name(#rust_method_params) -> #returns {
				let params = { #parameter_builder };
				self.#called_method(#rpc_method_name, params)
			}
		};
		Ok(method)
	}

	fn render_sub(&self, sub: &RpcSubscription) -> Result<TokenStream2, syn::Error> {
		// `jsonrpsee::core::ClientError`
		let jrps_error = self.jrps_client_item(quote! { core::client::Error });
		// Rust method to invoke (e.g. `self.<foo>(...)`).
		let rust_method_name = &sub.signature.sig.ident;
		// List of inputs to put into `Params` (e.g. `self.foo(<12, "baz">)`).
		let rust_method_params = &sub.signature.sig.inputs;
		// Name of the RPC subscription (e.g. `foo_sub`).
		let rpc_sub_name = self.rpc_identifier(&sub.name);
		// Name of the RPC method to unsubscribe (e.g. `foo_unsub`).
		let rpc_unsub_name = self.rpc_identifier(&sub.unsubscribe);

		// `returns` represent the return type of the *rust method*, which is wrapped
		// into the `Subscription` object.
		let sub_type = self.jrps_client_item(quote! { core::client::Subscription });
		let item = &sub.item;
		let returns = quote! { impl core::future::Future<Output = Result<#sub_type<#item>, #jrps_error>> + Send };

		// Encoded parameters for the request.
		let parameter_builder = self.encode_params(&sub.params, &sub.param_kind, &sub.signature);
		// Doc-comment to be associated with the method.
		let docs = &sub.docs;

		let method = quote! {
			#docs
			#[allow(non_snake_case)]
			#[allow(clippy::used_underscore_binding)]
			fn #rust_method_name(#rust_method_params) -> #returns {
				let params = #parameter_builder;
				self.subscribe(#rpc_sub_name, params, #rpc_unsub_name)
			}
		};
		Ok(method)
	}

	fn encode_params(&self, params: &[RpcFnArg], param_kind: &ParamKind, signature: &syn::TraitItemFn) -> TokenStream2 {
		const ILLEGAL_PARAM_NAME: &str = "__RpcParams__";

		let jsonrpsee = self.jsonrpsee_client_path.as_ref().unwrap();
		let p = Ident::new(ILLEGAL_PARAM_NAME, proc_macro2::Span::call_site());

		let reexports = self.jrps_client_item(quote! { core::__reexports });

		if params.is_empty() {
			return quote!({
				#jsonrpsee::core::params::ArrayParams::new()
			});
		}

		if params.iter().any(|arg| arg.arg_pat().ident == p) {
			panic!(
				"Cannot use `{}` as a parameter name because it's overlapping with an internal variable in the generated code. Change it something else to make it work",
				ILLEGAL_PARAM_NAME
			);
		}

		match param_kind {
			ParamKind::Map => {
				// Extract parameter names.
				let param_names = extract_param_names(&signature.sig);
				// Combine parameter names and values to pass them as parameters.
				let params_insert = params.iter().map(|arg| {
					// Throw away the type.
					let value = arg.arg_pat();
					let name = arg.name();
					quote!(#name, #value)
				});

				// It's possible that the user has a parameter named `ILLEGAL_PARAM_NAME` in there API
				// which would conflict with our internal parameter name
				//
				// We will throw an error if that is the case.
				if param_names.iter().any(|name| name == ILLEGAL_PARAM_NAME) {
					panic!("Cannot use `{}` as a parameter name", ILLEGAL_PARAM_NAME);
				}

				quote!({
					let mut #p = #jsonrpsee::core::params::ObjectParams::new();
					#(
						if let Err(err) = #p.insert(#params_insert) {
							#reexports::panic_fail_serialize(stringify!(#params_insert), err);
						}
					)*
					#p
				})
			}
			ParamKind::Array => {
				// Throw away the type.
				let params = params.iter().map(RpcFnArg::arg_pat);

				quote!({
					let mut #p = #jsonrpsee::core::params::ArrayParams::new();
					#(
						if let Err(err) = #p.insert(#params) {
							#reexports::panic_fail_serialize(stringify!(#params), err);
						}
					)*
					#p
				})
			}
		}
	}
}

fn extract_param_names(sig: &syn::Signature) -> Vec<String> {
	sig.inputs
		.iter()
		.filter_map(|param| match param {
			FnArg::Typed(PatType { pat, .. }) => match &**pat {
				Pat::Ident(PatIdent { ident, .. }) => Some(ident.to_string()),
				_ => None,
			},
			_ => None,
		})
		.collect()
}
