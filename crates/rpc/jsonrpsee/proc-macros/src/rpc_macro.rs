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

//! Declaration of the JSON RPC generator procedural macros.

use std::borrow::Cow;

use crate::attributes::{
	Aliases, Argument, AttributeMeta, MissingArgument, NameMapping, ParamKind, optional, parse_param_kind,
};
use crate::helpers::extract_doc_comments;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, Token, punctuated::Punctuated};

/// Represents a single argument in a RPC call.
///
/// stores modifications based on attributes
#[derive(Debug, Clone)]
pub struct RpcFnArg {
	pub(crate) arg_pat: syn::PatIdent,
	rename_to: Option<String>,
	pub(crate) ty: syn::Type,
}

impl RpcFnArg {
	pub fn from_arg_attrs(arg_pat: syn::PatIdent, ty: syn::Type, attrs: &mut Vec<syn::Attribute>) -> syn::Result<Self> {
		let mut rename_to = None;

		if let Some(attr) = find_attr(attrs, "argument") {
			let [rename] = AttributeMeta::parse(attr.clone())?.retain(["rename"])?;

			let rename = optional(rename, Argument::string)?;

			if let Some(rename) = rename {
				rename_to = Some(rename);
			}
		}

		// remove argument attribute after inspection
		attrs.retain(|attr| !attr.meta.path().is_ident("argument"));

		Ok(Self { arg_pat, rename_to, ty })
	}

	/// Return the pattern identifier of the argument.
	pub fn arg_pat(&self) -> &syn::PatIdent {
		&self.arg_pat
	}
	/// Return the string representation of this argument when (de)seriaizing.
	pub fn name(&self) -> String {
		self.rename_to.clone().unwrap_or_else(|| self.arg_pat.ident.to_string())
	}
	/// Return the type of the argument.
	pub fn ty(&self) -> &syn::Type {
		&self.ty
	}
}

#[derive(Debug, Clone)]
pub struct RpcMethod {
	pub name: String,
	pub blocking: bool,
	pub docs: TokenStream2,
	pub deprecated: TokenStream2,
	pub params: Vec<RpcFnArg>,
	pub param_kind: ParamKind,
	pub returns: Option<syn::Type>,
	pub signature: syn::TraitItemFn,
	pub aliases: Vec<String>,
	pub with_extensions: bool,
}

impl RpcMethod {
	pub fn from_item(attr: Attribute, mut method: syn::TraitItemFn) -> syn::Result<Self> {
		let [aliases, blocking, name, param_kind, with_extensions] =
			AttributeMeta::parse(attr)?.retain(["aliases", "blocking", "name", "param_kind", "with_extensions"])?;

		let aliases = parse_aliases(aliases)?;
		let blocking = optional(blocking, Argument::flag)?.is_some();
		let name = name?.string()?;
		let param_kind = parse_param_kind(param_kind)?;
		let with_extensions = optional(with_extensions, Argument::flag)?.is_some();

		let docs = extract_doc_comments(&method.attrs);
		let deprecated = match find_attr(&method.attrs, "deprecated") {
			Some(attr) => quote!(#attr),
			None => quote!(),
		};

		if blocking && method.sig.asyncness.is_some() {
			return Err(syn::Error::new(method.sig.span(), "Blocking method must be synchronous"));
		}

		let params: Vec<_> = method
			.sig
			.inputs
			.iter_mut()
			.filter_map(|arg| match arg {
				syn::FnArg::Receiver(_) => None,
				syn::FnArg::Typed(arg) => match &*arg.pat {
					syn::Pat::Ident(name) => {
						Some(RpcFnArg::from_arg_attrs(name.clone(), (*arg.ty).clone(), &mut arg.attrs))
					}
					syn::Pat::Wild(wild) => Some(Err(syn::Error::new(
						wild.underscore_token.span(),
						"Method argument names must be valid Rust identifiers; got `_` instead",
					))),
					_ => Some(Err(syn::Error::new(
						arg.span(),
						format!("Unexpected method signature input; got {:?} ", *arg.pat),
					))),
				},
			})
			.collect::<Result<_, _>>()?;

		let returns = match method.sig.output.clone() {
			syn::ReturnType::Default => None,
			syn::ReturnType::Type(_, output) => Some(*output),
		};

		// We've analyzed attributes and don't need them anymore.
		method.attrs.clear();

		Ok(Self {
			aliases,
			blocking,
			name,
			params,
			param_kind,
			returns,
			signature: method,
			docs,
			deprecated,
			with_extensions,
		})
	}
}

#[derive(Debug, Clone)]
pub struct RpcSubscription {
	pub name: String,
	/// When subscribing to an RPC, users can override the content of the `method` field
	/// in the JSON data sent to subscribers.
	/// Each subscription thus has one method name to set up the subscription,
	/// one to unsubscribe and, optionally, a third method name used to describe the
	/// payload (aka "notification") sent back from the server to subscribers.
	/// If no override is provided, the subscription method name is used.
	pub notif_name_override: Option<String>,
	pub docs: TokenStream2,
	pub unsubscribe: String,
	pub params: Vec<RpcFnArg>,
	pub param_kind: ParamKind,
	pub item: syn::Type,
	pub signature: syn::TraitItemFn,
	pub aliases: Vec<String>,
	pub unsubscribe_aliases: Vec<String>,
	pub with_extensions: bool,
}

impl RpcSubscription {
	pub fn from_item(attr: syn::Attribute, mut sub: syn::TraitItemFn) -> syn::Result<Self> {
		let [aliases, item, name, param_kind, unsubscribe, unsubscribe_aliases, with_extensions] =
			AttributeMeta::parse(attr)?.retain([
				"aliases",
				"item",
				"name",
				"param_kind",
				"unsubscribe",
				"unsubscribe_aliases",
				"with_extensions",
			])?;

		let aliases = parse_aliases(aliases)?;
		let map = name?.value::<NameMapping>()?;
		let name = map.name;
		let notif_name_override = map.mapped;
		let item = item?.value()?;
		let param_kind = parse_param_kind(param_kind)?;
		let unsubscribe_aliases = parse_aliases(unsubscribe_aliases)?;
		let with_extensions = optional(with_extensions, Argument::flag)?.is_some();

		let docs = extract_doc_comments(&sub.attrs);
		let unsubscribe = match parse_subscribe(unsubscribe)? {
			Some(unsub) => unsub,
			None => build_unsubscribe_method(&name).unwrap_or_else(||
				panic!("Could not generate the unsubscribe method with name '{name}'. You need to provide the name manually using the `unsubscribe` attribute in your RPC API definition"),
			),
		};

		let params: Vec<_> = sub
			.sig
			.inputs
			.iter_mut()
			.filter_map(|arg| match arg {
				syn::FnArg::Receiver(_) => None,
				syn::FnArg::Typed(arg) => match &*arg.pat {
					syn::Pat::Ident(name) => {
						Some(RpcFnArg::from_arg_attrs(name.clone(), (*arg.ty).clone(), &mut arg.attrs))
					}
					_ => panic!("Identifier in signature must be an ident"),
				},
			})
			.collect::<Result<_, _>>()?;

		// We've analyzed attributes and don't need them anymore.
		sub.attrs.clear();

		Ok(Self {
			name,
			notif_name_override,
			unsubscribe,
			unsubscribe_aliases,
			params,
			param_kind,
			item,
			signature: sub,
			aliases,
			docs,
			with_extensions,
		})
	}
}

#[derive(Debug)]
pub struct RpcDescription {
	/// Path to the `jsonrpsee` client types part.
	pub(crate) jsonrpsee_client_path: Option<TokenStream2>,
	/// Path to the `jsonrpsee` server types part.
	pub(crate) jsonrpsee_server_path: Option<TokenStream2>,
	/// Switch denoting that server trait must be generated.
	/// Assuming that trait to which attribute is applied is named `Foo`, the generated
	/// server trait will have `FooServer` name.
	pub(crate) needs_server: bool,
	/// Switch denoting that client extension trait must be generated.
	/// Assuming that trait to which attribute is applied is named `Foo`, the generated
	/// client trait will have `FooClient` name.
	pub(crate) needs_client: bool,
	/// Optional prefix for RPC namespace.
	pub(crate) namespace: Option<String>,
	/// Optional separator between namespace and method name. Defaults to `_`.
	pub(crate) namespace_separator: Option<String>,
	/// Trait definition in which all the attributes were stripped.
	pub(crate) trait_def: syn::ItemTrait,
	/// List of RPC methods defined in the trait.
	pub(crate) methods: Vec<RpcMethod>,
	/// List of RPC subscriptions defined in the trait.
	pub(crate) subscriptions: Vec<RpcSubscription>,
	/// Optional user defined trait bounds for the client implementation.
	pub(crate) client_bounds: Option<Punctuated<syn::WherePredicate, Token![,]>>,
	/// Optional user defined trait bounds for the server implementation.
	pub(crate) server_bounds: Option<Punctuated<syn::WherePredicate, Token![,]>>,
}

impl RpcDescription {
	pub fn from_item(attr: Attribute, mut item: syn::ItemTrait) -> syn::Result<Self> {
		let [client, server, namespace, namespace_separator, client_bounds, server_bounds] =
			AttributeMeta::parse(attr)?.retain([
				"client",
				"server",
				"namespace",
				"namespace_separator",
				"client_bounds",
				"server_bounds",
			])?;

		let needs_server = optional(server, Argument::flag)?.is_some();
		let needs_client = optional(client, Argument::flag)?.is_some();
		let namespace = optional(namespace, Argument::string)?;
		let namespace_separator = optional(namespace_separator, Argument::string)?;
		let client_bounds = optional(client_bounds, Argument::group)?;
		let server_bounds = optional(server_bounds, Argument::group)?;
		if !needs_server && !needs_client {
			return Err(syn::Error::new_spanned(&item.ident, "Either 'server' or 'client' attribute must be applied"));
		}

		if client_bounds.is_some() && !needs_client {
			return Err(syn::Error::new_spanned(
				&item.ident,
				"Attribute 'client' must be specified with 'client_bounds'",
			));
		}

		if server_bounds.is_some() && !needs_server {
			return Err(syn::Error::new_spanned(
				&item.ident,
				"Attribute 'server' must be specified with 'server_bounds'",
			));
		}

		let jsonrpsee_client_path = crate::helpers::find_jsonrpsee_client_crate().ok();
		let jsonrpsee_server_path = crate::helpers::find_jsonrpsee_server_crate().ok();

		if needs_client && jsonrpsee_client_path.is_none() {
			return Err(syn::Error::new_spanned(&item.ident, "Unable to locate 'jsonrpsee' client dependency"));
		}
		if needs_server && jsonrpsee_server_path.is_none() {
			return Err(syn::Error::new_spanned(&item.ident, "Unable to locate 'jsonrpsee' server dependency"));
		}

		item.attrs.clear(); // Remove RPC attributes.

		let mut methods = Vec::new();
		let mut subscriptions = Vec::new();

		// Go through all the methods in the trait and collect methods and
		// subscriptions.
		for entry in item.items.iter() {
			if let syn::TraitItem::Fn(method) = entry {
				if method.sig.receiver().is_none() {
					return Err(syn::Error::new_spanned(&method.sig, "First argument of the trait must be '&self'"));
				}

				let mut is_method = false;
				let mut is_sub = false;
				if let Some(attr) = find_attr(&method.attrs, "method") {
					is_method = true;

					let method_data = RpcMethod::from_item(attr.clone(), method.clone())?;

					methods.push(method_data);
				}
				if let Some(attr) = find_attr(&method.attrs, "subscription") {
					is_sub = true;
					if is_method {
						return Err(syn::Error::new_spanned(
							method,
							"Element cannot be both subscription and method at the same time",
						));
					}

					let sub_data = RpcSubscription::from_item(attr.clone(), method.clone())?;
					subscriptions.push(sub_data);
				}

				if !is_method && !is_sub {
					return Err(syn::Error::new_spanned(
						method,
						"Methods must have either 'method' or 'subscription' attribute",
					));
				}
			} else {
				return Err(syn::Error::new_spanned(entry, "Only methods allowed in RPC traits"));
			}
		}

		if methods.is_empty() && subscriptions.is_empty() {
			return Err(syn::Error::new_spanned(&item, "RPC cannot be empty"));
		}

		Ok(Self {
			jsonrpsee_client_path,
			jsonrpsee_server_path,
			needs_server,
			needs_client,
			namespace,
			namespace_separator,
			trait_def: item,
			methods,
			subscriptions,
			client_bounds,
			server_bounds,
		})
	}

	pub fn render(self) -> Result<TokenStream2, syn::Error> {
		let server_impl = if self.needs_server { self.render_server()? } else { TokenStream2::new() };
		let client_impl = if self.needs_client { self.render_client()? } else { TokenStream2::new() };

		Ok(quote! {
			#server_impl
			#client_impl
		})
	}

	/// Formats the identifier as a path relative to the resolved
	/// `jsonrpsee` client path.
	pub(crate) fn jrps_client_item(&self, item: impl quote::ToTokens) -> TokenStream2 {
		let jsonrpsee = self.jsonrpsee_client_path.as_ref().unwrap();
		quote! { #jsonrpsee::#item }
	}

	/// Formats the identifier as a path relative to the resolved
	/// `jsonrpsee` server path.
	pub(crate) fn jrps_server_item(&self, item: impl quote::ToTokens) -> TokenStream2 {
		let jsonrpsee = self.jsonrpsee_server_path.as_ref().unwrap();
		quote! { #jsonrpsee::#item }
	}

	/// Based on the namespace and separator, renders the full name of the RPC method/subscription.
	/// Examples:
	/// For namespace `foo`, method `makeSpam`, and separator `_`, result will be `foo_makeSpam`.
	/// For separator `.`, result will be `foo.makeSpam`.
	/// For no namespace, returns just `makeSpam`.
	pub(crate) fn rpc_identifier<'a>(&self, method: &'a str) -> Cow<'a, str> {
		if let Some(ns) = &self.namespace {
			let sep = self.namespace_separator.as_deref().unwrap_or("_");
			format!("{ns}{sep}{method}").into()
		} else {
			Cow::Borrowed(method)
		}
	}
}

fn parse_aliases(arg: Result<Argument, MissingArgument>) -> syn::Result<Vec<String>> {
	let aliases = optional(arg, Argument::value::<Aliases>)?;

	Ok(aliases.map(|a| a.list.into_iter().map(|lit| lit.value()).collect()).unwrap_or_default())
}

fn parse_subscribe(arg: Result<Argument, MissingArgument>) -> syn::Result<Option<String>> {
	let unsub = optional(arg, Argument::string)?;

	Ok(unsub)
}

fn find_attr<'a>(attrs: &'a [Attribute], ident: &str) -> Option<&'a Attribute> {
	attrs.iter().find(|a| a.path().is_ident(ident))
}

fn build_unsubscribe_method(method: &str) -> Option<String> {
	method.strip_prefix("subscribe").map(|s| format!("unsubscribe{s}"))
}
