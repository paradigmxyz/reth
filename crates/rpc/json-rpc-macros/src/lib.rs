//! Procedural macros for `reth-json-rpc`.

use proc_macro::TokenStream;
use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote, ToTokens};
use std::collections::HashSet;
use syn::{
    bracketed, ext::IdentExt, meta::ParseNestedMeta, parenthesized, parse::Parse,
    parse_macro_input, parse_quote, Attribute, FnArg, GenericArgument, Ident, ItemTrait, LitStr,
    Pat, PathArguments, ReturnType, Token, TraitItem, TraitItemFn, Type, WherePredicate,
};

/// Generates `{Trait}Server` and `{Trait}Client` traits from an RPC API definition.
///
/// The attribute takes:
/// - `server`: generate `{Trait}Server` with an `into_rpc` method that builds an `RpcModule`.
/// - `client`: generate `{Trait}Client`, implemented for every `ClientT`, or every
///   `SubscriptionClientT` if the trait has subscriptions.
/// - `namespace = "ns"`: prefix method names with `ns_`.
/// - `namespace_separator = "."`: separate the namespace with `.` instead of `_`.
/// - `server_bounds(..)` / `client_bounds(..)`: replace the inferred bounds on generic parameters.
///
/// Every trait method needs one of:
/// - `#[method(name = "..", aliases = [".."])]`: async or sync method. Aliases have no namespace.
///   - `blocking`: run the sync method on the blocking thread pool.
/// - `#[subscription(name = "sub" => "notification", unsubscribe = "unsub", item = T)]`: the server
///   method also takes a `PendingSubscriptionSink` after `&self`. The notification name defaults to
///   the subscription name, and `unsubscribe` defaults to the name with its `subscribe` prefix
///   replaced by `unsubscribe`.
///   - `unsubscribe_aliases = [".."]`: aliases of the unsubscribe method, with no namespace.
///
/// Both take `with_extensions`, which passes the request's `&Extensions` to the server method
/// after `&self` and the subscription sink, and `param_kind = map`, which makes the client send
/// named parameters.
///
/// Parameters are positional, or named by their name or its `lowerCamelCase` form. A missing
/// parameter is read as `null`, so trailing `Option` parameters may be omitted. Methods take at
/// most 12 parameters. `#[argument(rename = "..")]` sets the parameter name.
#[proc_macro_attribute]
pub fn rpc(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut args = RpcArgs::default();
    let parser = syn::meta::parser(|meta| args.parse(meta));
    parse_macro_input!(attr with parser);
    let item = parse_macro_input!(item as ItemTrait);
    match Rpc::new(args, item) {
        Ok(rpc) => rpc.render().into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[derive(Default)]
struct RpcArgs {
    server: bool,
    client: bool,
    namespace: Option<String>,
    namespace_separator: Option<String>,
    server_bounds: Option<Vec<WherePredicate>>,
    client_bounds: Option<Vec<WherePredicate>>,
}

impl RpcArgs {
    fn parse(&mut self, meta: ParseNestedMeta<'_>) -> syn::Result<()> {
        if meta.path.is_ident("server") {
            self.server = true;
        } else if meta.path.is_ident("client") {
            self.client = true;
        } else if meta.path.is_ident("namespace") {
            self.namespace = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("namespace_separator") {
            self.namespace_separator = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if meta.path.is_ident("server_bounds") {
            self.server_bounds = Some(parse_bounds(&meta)?);
        } else if meta.path.is_ident("client_bounds") {
            self.client_bounds = Some(parse_bounds(&meta)?);
        } else {
            return Err(meta.error("unknown argument"))
        }
        Ok(())
    }
}

struct Rpc {
    args: RpcArgs,
    item: ItemTrait,
    methods: Vec<Method>,
}

struct Method {
    func: TraitItemFn,
    name: String,
    aliases: Vec<String>,
    args: Vec<Arg>,
    with_extensions: bool,
    named_params: bool,
    kind: Kind,
}

struct Arg {
    ident: Ident,
    ty: Type,
    rename: Option<String>,
}

enum Kind {
    Method {
        blocking: bool,
    },
    Subscription {
        notification: String,
        unsubscribe: String,
        unsubscribe_aliases: Vec<String>,
        item: Box<Type>,
    },
}

impl Rpc {
    fn new(args: RpcArgs, mut item: ItemTrait) -> syn::Result<Self> {
        if !args.server && !args.client {
            return Err(syn::Error::new_spanned(&item.ident, "expected `server` or `client`"))
        }
        let separator = args.namespace_separator.as_deref().unwrap_or("_");
        let ns = |name: &str| match &args.namespace {
            Some(ns) => format!("{ns}{separator}{name}"),
            None => name.to_owned(),
        };

        let mut names = HashSet::new();
        let mut methods = Vec::new();
        for item in &mut item.items {
            let TraitItem::Fn(func) = item else {
                return Err(syn::Error::new_spanned(item, "only methods are allowed in RPC traits"));
            };
            if func.sig.receiver().is_none() {
                return Err(syn::Error::new_spanned(&func.sig, "expected a `&self` receiver"))
            }
            let MethodAttr { name, aliases, with_extensions, named_params, kind } =
                parse_method_attr(func)?;
            let (name, kind) = match kind {
                Kind::Method { blocking } => {
                    if blocking && func.sig.asyncness.is_some() {
                        return Err(syn::Error::new_spanned(
                            &func.sig,
                            "blocking methods must not be async",
                        ))
                    }
                    (ns(&name), Kind::Method { blocking })
                }
                Kind::Subscription { notification, unsubscribe, unsubscribe_aliases, item } => {
                    let unsubscribe = ns(&unsubscribe);
                    for name in std::iter::once(&unsubscribe).chain(&unsubscribe_aliases) {
                        if !names.insert(name.clone()) {
                            return Err(duplicate(&func.sig.ident, name))
                        }
                    }
                    let notification = ns(&notification);
                    (
                        ns(&name),
                        Kind::Subscription { notification, unsubscribe, unsubscribe_aliases, item },
                    )
                }
            };
            for name in std::iter::once(&name).chain(&aliases) {
                if !names.insert(name.clone()) {
                    return Err(duplicate(&func.sig.ident, name))
                }
            }
            let args = func
                .sig
                .inputs
                .iter_mut()
                .skip(1)
                .map(|arg| {
                    if let FnArg::Typed(arg) = arg &&
                        let Pat::Ident(pat) = &*arg.pat
                    {
                        let rename = parse_argument_attr(&mut arg.attrs)?;
                        return Ok(Arg { ident: pat.ident.clone(), ty: (*arg.ty).clone(), rename })
                    }
                    Err(syn::Error::new_spanned(arg, "expected an identifier"))
                })
                .collect::<syn::Result<Vec<_>>>()?;
            if args.len() > 12 {
                return Err(syn::Error::new_spanned(&func.sig, "expected at most 12 parameters"))
            }
            methods.push(Method {
                func: func.clone(),
                name,
                aliases,
                args,
                with_extensions,
                named_params,
                kind,
            });
        }

        Ok(Self { args, item, methods })
    }

    fn render(&self) -> TokenStream2 {
        let server = self.args.server.then(|| self.render_server());
        let client = self.args.client.then(|| self.render_client());
        quote!(#server #client)
    }

    fn render_server(&self) -> TokenStream2 {
        let ItemTrait { attrs, vis, ident, generics, .. } = &self.item;
        let name = format_ident!("{ident}Server");
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
        let doc = default_doc(attrs, &format!("Server trait for the `{ident}` RPC API."));
        let bounds = self.bounds(self.args.server_bounds.as_ref(), false);

        let fns = self.methods.iter().map(|m| {
            let mut func = m.func.clone();
            if m.with_extensions {
                func.sig.inputs.insert(1, parse_quote!(ext: &::reth_json_rpc::Extensions));
            }
            if matches!(m.kind, Kind::Subscription { .. }) {
                func.sig.inputs.insert(
                    1,
                    parse_quote!(subscription_sink: ::reth_json_rpc::PendingSubscriptionSink),
                );
            }
            if func.sig.asyncness.take().is_some() {
                let output = match &func.sig.output {
                    ReturnType::Default => quote!(()),
                    ReturnType::Type(_, ty) => ty.to_token_stream(),
                };
                func.sig.output = parse_quote! {
                    -> impl ::core::future::Future<Output = #output> + ::core::marker::Send
                };
                if let Some(block) = &mut func.default {
                    *block = parse_quote!({ async move #block });
                }
            }
            func
        });

        let registrations = self.methods.iter().map(|m| {
            let rpc_name = &m.name;
            let rust_name = &m.func.sig.ident;
            let call = quote!(<Self as #name #ty_generics>::#rust_name);
            let is_async = m.func.sig.asyncness.is_some();
            let wait = is_async.then(|| quote!(.await));
            let args = m.args.iter().map(|arg| &arg.ident).collect::<Vec<_>>();
            let params = if m.args.is_empty() { quote!(_) } else { quote!(__params) };
            let ext = if m.with_extensions { quote!(__ext) } else { quote!(_) };
            let ext_arg = m.with_extensions.then(|| quote!(, &__ext));
            let mut unsubscribe_aliases = None;
            let pat = quote!((#(#args,)*));
            let parse = parse_args(&m.args);
            let register = match &m.kind {
                Kind::Method { blocking } => {
                    let body = quote!(#call(&__ctx #ext_arg #(, #args)*) #wait);
                    if is_async && m.args.is_empty() {
                        quote! {
                            rpc.register_async_method(#rpc_name, move |_, #ext| {
                                let __ctx = __ctx.clone();
                                async move { #body }
                            })
                        }
                    } else if is_async {
                        quote! {
                            rpc.register_async_method_or_err(#rpc_name, move |__params, #ext| {
                                let __ctx = __ctx.clone();
                                #parse.map(move |#pat| async move { #body })
                            })
                        }
                    } else {
                        let register = if *blocking {
                            quote!(register_blocking_method)
                        } else {
                            quote!(register_method)
                        };
                        let body = if m.args.is_empty() {
                            body
                        } else {
                            quote!(::reth_json_rpc::__private::MethodResult(#parse.map(|#pat| #body)))
                        };
                        quote!(rpc.#register(#rpc_name, move |#params, #ext| #body))
                    }
                }
                Kind::Subscription {
                    notification, unsubscribe, unsubscribe_aliases: a, ..
                } => {
                    unsubscribe_aliases = Some((unsubscribe, a));
                    let parse = (!m.args.is_empty()).then(|| {
                        quote! {
                            let #pat = match #parse {
                                ::core::result::Result::Ok(__args) => __args,
                                ::core::result::Result::Err(__err) => {
                                    __pending.reject(__err);
                                    return ::core::result::Result::Ok(())
                                }
                            };
                        }
                    });
                    quote! {
                        rpc.register_subscription(
                            #rpc_name, #notification, #unsubscribe,
                            move |#params, __pending, #ext| {
                                let __ctx = __ctx.clone();
                                async move {
                                    #parse
                                    #call(&__ctx, __pending #ext_arg #(, #args)*) #wait
                                }
                            },
                        )
                    }
                }
            };
            let aliases = &m.aliases;
            let unsubscribe_aliases = unsubscribe_aliases.map(|(unsubscribe, aliases)| {
                quote!(#(let _ = rpc.register_alias(#aliases, #unsubscribe);)*)
            });
            // Names are checked for duplicates above, so registration cannot fail.
            quote! {
                {
                    let __ctx = __ctx.clone();
                    let _ = #register;
                }
                #(let _ = rpc.register_alias(#aliases, #rpc_name);)*
                #unsubscribe_aliases
            }
        });

        quote! {
            #(#attrs)*
            #doc
            #vis trait #name #impl_generics:
                ::core::marker::Sized + ::core::marker::Send + ::core::marker::Sync + 'static
            #where_clause
            {
                #(#fns)*

                /// Collects all methods and subscriptions into an `RpcModule`.
                #[allow(deprecated)]
                fn into_rpc(self) -> ::reth_json_rpc::RpcModule
                where
                    #(#bounds,)*
                {
                    let __ctx = ::std::sync::Arc::new(self);
                    let mut rpc = ::reth_json_rpc::RpcModule::new();
                    #(#registrations)*
                    rpc
                }
            }
        }
    }

    fn render_client(&self) -> TokenStream2 {
        let ItemTrait { vis, ident, generics, .. } = &self.item;
        let name = format_ident!("{ident}Client");
        let doc = format!("Client for the `{ident}` RPC API.");
        let (impl_generics, ty_generics, _) = generics.split_for_impl();
        let mut blanket = generics.clone();
        blanket.params.push(parse_quote!(__Client));
        let (blanket_generics, _, _) = blanket.split_for_impl();
        let bounds = generics
            .where_clause
            .iter()
            .flat_map(|clause| clause.predicates.iter().cloned())
            .chain(self.bounds(self.args.client_bounds.as_ref(), true))
            .collect::<Vec<_>>();
        let has_subscriptions =
            self.methods.iter().any(|m| matches!(m.kind, Kind::Subscription { .. }));
        let super_trait = if has_subscriptions {
            quote!(::reth_json_rpc::client::SubscriptionClientT)
        } else {
            quote!(::reth_json_rpc::client::ClientT)
        };

        let fns = self.methods.iter().map(|m| {
            let rpc_name = &m.name;
            let mut sig = m.func.sig.clone();
            sig.asyncness = None;
            let (output, func, unsubscribe) = match &m.kind {
                Kind::Method { .. } => match result_ok_type(&sig.output) {
                    Ok(ty) => {
                        (quote!(#ty), quote!(ClientT::request::<#ty, _>), None)
                    }
                    Err(err) => return err.to_compile_error(),
                },
                Kind::Subscription { unsubscribe, item, .. } => (
                    quote!(::reth_json_rpc::client::Subscription<#item>),
                    quote!(SubscriptionClientT::subscribe::<#item, _>),
                    Some(quote!(, #unsubscribe)),
                ),
            };
            let call =
                |params| quote!(::reth_json_rpc::client::#func(self, #rpc_name, #params #unsubscribe));
            sig.output = parse_quote! {
                -> impl ::core::future::Future<
                    Output = ::core::result::Result<#output, ::reth_json_rpc::client::Error>,
                > + ::core::marker::Send
            };
            let attrs = &m.func.attrs;
            let body = if m.named_params {
                let inserts = m.args.iter().map(|arg| {
                    let ident = &arg.ident;
                    let name = arg.rename.clone().unwrap_or_else(|| ident.unraw().to_string());
                    quote!(__rpc_params.insert(#name, #ident))
                });
                let call = call(quote!(__rpc_params));
                quote! {
                    async move {
                        #[allow(unused_mut)]
                        let mut __rpc_params = ::reth_json_rpc::client::ObjectParams::new();
                        #(
                            if let ::core::result::Result::Err(__err) = #inserts {
                                return ::core::result::Result::Err(
                                    ::reth_json_rpc::client::Error::ParseError(__err),
                                );
                            }
                        )*
                        #call.await
                    }
                }
            } else {
                let args = m.args.iter().map(|arg| &arg.ident);
                call(quote!((#(#args,)*)))
            };
            quote! {
                #(#attrs)*
                #[allow(non_snake_case, clippy::used_underscore_binding)]
                #sig {
                    #body
                }
            }
        });

        quote! {
            #[doc = #doc]
            #vis trait #name #impl_generics: #super_trait where #(#bounds,)* {
                #(#fns)*
            }

            impl #blanket_generics #name #ty_generics for __Client
            where
                __Client: #super_trait,
                #(#bounds,)*
            {
            }
        }
    }

    /// Returns `custom`, or bounds for each type parameter inferred from where it appears.
    ///
    /// Every type parameter is `Send + Sync + 'static`. Parameters in arguments must be
    /// serializable on the client and deserializable on the server, and the reverse for parameters
    /// in return and subscription item types.
    fn bounds(&self, custom: Option<&Vec<WherePredicate>>, client: bool) -> Vec<WherePredicate> {
        if let Some(custom) = custom {
            return custom.clone()
        }
        let mut inputs = HashSet::new();
        let mut outputs = HashSet::new();
        for m in &self.methods {
            for arg in &m.args {
                collect_idents(arg.ty.to_token_stream(), &mut inputs);
            }
            match &m.kind {
                Kind::Method { .. } => {
                    collect_idents(m.func.sig.output.to_token_stream(), &mut outputs)
                }
                Kind::Subscription { item, .. } => {
                    collect_idents(item.to_token_stream(), &mut outputs)
                }
            }
        }
        let (de, ser) = if client { (&outputs, &inputs) } else { (&inputs, &outputs) };
        self.item
            .generics
            .type_params()
            .map(|param| {
                let ident = &param.ident;
                let de = de.contains(ident).then(|| quote!(+ ::reth_json_rpc::DeserializeOwned));
                let ser = ser.contains(ident).then(|| quote!(+ ::reth_json_rpc::Serialize));
                parse_quote! {
                    #ident: ::core::marker::Send + ::core::marker::Sync + 'static #de #ser
                }
            })
            .collect()
    }
}

struct MethodAttr {
    name: String,
    aliases: Vec<String>,
    with_extensions: bool,
    named_params: bool,
    kind: Kind,
}

/// Removes the `#[method]` or `#[subscription]` attribute from `func` and parses it.
fn parse_method_attr(func: &mut TraitItemFn) -> syn::Result<MethodAttr> {
    let index = func
        .attrs
        .iter()
        .position(|attr| attr.path().is_ident("method") || attr.path().is_ident("subscription"))
        .ok_or_else(|| {
            syn::Error::new_spanned(&func.sig, "expected `#[method]` or `#[subscription]`")
        })?;
    let attr = func.attrs.remove(index);
    let is_subscription = attr.path().is_ident("subscription");

    let mut name = None;
    let mut notification = None;
    let mut aliases = Vec::new();
    let mut unsubscribe = None;
    let mut unsubscribe_aliases = Vec::new();
    let mut item = None;
    let mut blocking = false;
    let mut with_extensions = false;
    let mut named_params = false;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            let input = meta.value()?;
            name = Some(input.parse::<LitStr>()?.value());
            if is_subscription && input.peek(Token![=>]) {
                input.parse::<Token![=>]>()?;
                notification = Some(input.parse::<LitStr>()?.value());
            }
        } else if meta.path.is_ident("aliases") {
            aliases = parse_names(&meta)?;
        } else if meta.path.is_ident("with_extensions") {
            with_extensions = true;
        } else if meta.path.is_ident("param_kind") {
            let kind = meta.value()?.parse::<Ident>()?;
            named_params = match kind.to_string().as_str() {
                "map" => true,
                "array" => false,
                _ => return Err(syn::Error::new_spanned(kind, "expected `map` or `array`")),
            };
        } else if !is_subscription && meta.path.is_ident("blocking") {
            blocking = true;
        } else if is_subscription && meta.path.is_ident("unsubscribe_aliases") {
            unsubscribe_aliases = parse_names(&meta)?;
        } else if is_subscription && meta.path.is_ident("unsubscribe") {
            unsubscribe = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if is_subscription && meta.path.is_ident("item") {
            item = Some(Box::new(meta.value()?.parse::<Type>()?));
        } else {
            return Err(meta.error("unknown argument"))
        }
        Ok(())
    })?;

    let name = name.ok_or_else(|| syn::Error::new_spanned(&attr, "missing `name`"))?;
    if !is_subscription {
        let kind = Kind::Method { blocking };
        return Ok(MethodAttr { name, aliases, with_extensions, named_params, kind })
    }
    let item = item.ok_or_else(|| syn::Error::new_spanned(&attr, "missing `item`"))?;
    let unsubscribe = unsubscribe
        .or_else(|| name.strip_prefix("subscribe").map(|rest| format!("unsubscribe{rest}")))
        .ok_or_else(|| syn::Error::new_spanned(&attr, "missing `unsubscribe`"))?;
    let notification = notification.unwrap_or_else(|| name.clone());
    let kind = Kind::Subscription { notification, unsubscribe, unsubscribe_aliases, item };
    Ok(MethodAttr { name, aliases, with_extensions, named_params, kind })
}

/// Removes the `#[argument]` attributes from a parameter and returns its `rename`, if any.
fn parse_argument_attr(attrs: &mut Vec<Attribute>) -> syn::Result<Option<String>> {
    let mut rename = None;
    for attr in attrs.extract_if(.., |attr| attr.path().is_ident("argument")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                rename = Some(meta.value()?.parse::<LitStr>()?.value());
                Ok(())
            } else {
                Err(meta.error("unknown argument"))
            }
        })?;
    }
    Ok(rename)
}

fn parse_names(meta: &ParseNestedMeta<'_>) -> syn::Result<Vec<String>> {
    let input = meta.value()?;
    let content;
    bracketed!(content in input);
    Ok(content
        .parse_terminated(<LitStr as Parse>::parse, Token![,])?
        .iter()
        .map(LitStr::value)
        .collect())
}

fn parse_bounds(meta: &ParseNestedMeta<'_>) -> syn::Result<Vec<WherePredicate>> {
    let content;
    parenthesized!(content in meta.input);
    Ok(content.parse_terminated(WherePredicate::parse, Token![,])?.into_iter().collect())
}

/// Generates an expression that parses `__params` into a tuple of the arguments.
fn parse_args(args: &[Arg]) -> TokenStream2 {
    let tys = args.iter().map(|arg| &arg.ty);
    let names = args.iter().map(|arg| {
        if let Some(rename) = &arg.rename {
            return quote!(&[#rename])
        }
        let name = arg.ident.unraw().to_string();
        let camel = lower_camel_case(&name);
        if camel == name {
            quote!(&[#name])
        } else {
            quote!(&[#name, #camel])
        }
    });
    quote! {
        <(#(#tys,)*) as ::reth_json_rpc::__private::FromParams<'_>>::from_params(
            &__params,
            &[#(#names),*],
        )
    }
}

/// Converts a `snake_case` name to `lowerCamelCase`.
fn lower_camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.trim_start_matches('_').chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Returns `T` from a `RpcResult<T>` or `Result<T, E>` return type.
fn result_ok_type(output: &ReturnType) -> syn::Result<&Type> {
    if let ReturnType::Type(_, ty) = output &&
        let Type::Path(path) = &**ty &&
        let Some(seg) = path.path.segments.last() &&
        (seg.ident == "RpcResult" || seg.ident == "Result") &&
        let PathArguments::AngleBracketed(args) = &seg.arguments &&
        let Some(GenericArgument::Type(ty)) = args.args.first()
    {
        return Ok(ty)
    }
    Err(syn::Error::new_spanned(output, "expected `RpcResult<T>` or `Result<T, E>`"))
}

fn collect_idents(tokens: TokenStream2, idents: &mut HashSet<Ident>) {
    for token in tokens {
        match token {
            TokenTree::Ident(ident) => {
                idents.insert(ident);
            }
            TokenTree::Group(group) => collect_idents(group.stream(), idents),
            _ => {}
        }
    }
}

fn default_doc(attrs: &[Attribute], doc: &str) -> Option<TokenStream2> {
    (!attrs.iter().any(|attr| attr.path().is_ident("doc"))).then(|| quote!(#[doc = #doc]))
}

fn duplicate(span: &Ident, name: &str) -> syn::Error {
    syn::Error::new_spanned(span, format!("`{name}` is already defined"))
}
