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

use std::{fmt, iter};

use proc_macro2::{Span, TokenStream as TokenStream2, TokenTree};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{Attribute, Error, LitStr, Token, spanned::Spanned};

pub(crate) struct AttributeMeta {
	pub path: syn::Path,
	pub arguments: Punctuated<Argument, Token![,]>,
}

pub(crate) struct Argument {
	pub label: syn::Ident,
	pub tokens: TokenStream2,
}

#[derive(Debug, Clone)]
pub enum ParamKind {
	Array,
	Map,
}

pub struct NameMapping {
	pub name: String,
	pub mapped: Option<String>,
}

pub struct Bracketed<T> {
	pub list: Punctuated<T, Token![,]>,
}

pub type Aliases = Bracketed<LitStr>;

impl Parse for Argument {
	fn parse(input: ParseStream) -> syn::Result<Self> {
		let label = input.parse()?;

		let mut scope = 0usize;

		// Need to read to till either the end of the stream,
		// or the nearest comma token that's not contained
		// inside angle brackets.
		let tokens = iter::from_fn(move || {
			if scope == 0 && input.peek(Token![,]) {
				return None;
			}

			if input.peek(Token![<]) {
				scope += 1;
			} else if input.peek(Token![>]) {
				scope = scope.saturating_sub(1);
			}

			input.parse::<TokenTree>().ok()
		})
		.collect();

		Ok(Argument { label, tokens })
	}
}

impl Parse for NameMapping {
	fn parse(input: ParseStream) -> syn::Result<Self> {
		let name = input.parse::<LitStr>()?.value();

		let mapped = if input.peek(Token![=>]) {
			input.parse::<Token![=>]>()?;

			Some(input.parse::<LitStr>()?.value())
		} else {
			None
		};

		Ok(NameMapping { name, mapped })
	}
}

impl<T: Parse> Parse for Bracketed<T> {
	fn parse(input: ParseStream) -> syn::Result<Self> {
		let content;

		syn::bracketed!(content in input);

		let list = content.parse_terminated(Parse::parse, Token![,])?;

		Ok(Bracketed { list })
	}
}

fn parenthesized<T: Parse>(input: ParseStream) -> syn::Result<Punctuated<T, Token![,]>> {
	let content;

	syn::parenthesized!(content in input);

	content.parse_terminated(T::parse, Token![,])
}

impl AttributeMeta {
	/// Parses `Attribute` with plain `TokenStream` into a more robust `AttributeMeta` with
	/// a collection `Arguments`.
	pub fn parse(attr: Attribute) -> syn::Result<AttributeMeta> {
		let path = attr.path().clone();

		let arguments = attr.parse_args_with(|input: ParseStream| input.parse_terminated(Parse::parse, Token![,]))?;

		Ok(AttributeMeta { path, arguments })
	}

	/// Attempt to get a list of `Argument`s from a list of names in order.
	///
	/// Errors if there is an argument with a name that's not on the list, or if there is a duplicate definition.
	pub fn retain<const N: usize>(self, allowed: [&str; N]) -> syn::Result<[Result<Argument, MissingArgument<'_>>; N]> {
		assert!(
			N != 0,
			"Calling `AttributeMeta::retain` with an empty `allowed` list, this is a bug, please report it"
		);

		let mut result: [Result<Argument, _>; N] = allowed.map(|name| Err(MissingArgument(self.path.span(), name)));

		for argument in self.arguments {
			if let Some(idx) = allowed.iter().position(|probe| argument.label == probe) {
				// If this position in the `result` array already contains an argument,
				// it means we got a duplicate definition
				if let Ok(old) = &result[idx] {
					return Err(Error::new(old.label.span(), format!("Duplicate argument `{}`", old.label)));
				}

				result[idx] = Ok(argument);
			} else {
				let mut err_str = format!("Unknown argument `{}`, expected one of: `", &argument.label);

				err_str.push_str(allowed[0]);
				err_str.extend(allowed[1..].iter().flat_map(|&label| ["`, `", label]));
				err_str.push('`');

				return Err(Error::new(argument.label.span(), err_str));
			}
		}

		Ok(result)
	}
}

pub(crate) struct MissingArgument<'a>(Span, &'a str);

impl fmt::Display for MissingArgument<'_> {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		let MissingArgument(_, missing) = self;

		write!(f, "Missing argument `{missing}`")
	}
}

impl From<MissingArgument<'_>> for Error {
	fn from(missing: MissingArgument) -> Self {
		Error::new(missing.0, missing)
	}
}

impl Argument {
	/// Asserts that the argument is just a simple `flag` without any value present
	pub fn flag(self) -> syn::Result<()> {
		if self.tokens.is_empty() { Ok(()) } else { Err(Error::new(self.tokens.span(), "Expected a flag argument")) }
	}

	/// Asserts that the argument is `key = value` pair and parses the value into `T`
	pub fn value<T: Parse>(self) -> syn::Result<T> {
		fn value_parser<T: Parse>(stream: ParseStream) -> syn::Result<T> {
			stream.parse::<Token![=]>()?;
			stream.parse()
		}

		value_parser.parse2(self.tokens)
	}

	pub fn group<T>(self) -> syn::Result<Punctuated<T, Token![,]>>
	where
		T: Parse,
	{
		parenthesized.parse2(self.tokens)
	}

	/// Asserts that the argument is `key = "string"` and gets the value of the string
	pub fn string(self) -> syn::Result<String> {
		self.value::<LitStr>().map(|lit| lit.value())
	}
}

pub(crate) fn optional<T, F>(arg: Result<Argument, MissingArgument>, transform: F) -> syn::Result<Option<T>>
where
	F: Fn(Argument) -> syn::Result<T>,
{
	arg.ok().map(transform).transpose()
}

pub(crate) fn parse_param_kind(arg: Result<Argument, MissingArgument>) -> syn::Result<ParamKind> {
	let kind: Option<syn::Ident> = optional(arg, Argument::value)?;

	match kind {
		None => Ok(ParamKind::Array),
		Some(ident) if ident == "array" => Ok(ParamKind::Array),
		Some(ident) if ident == "map" => Ok(ParamKind::Map),
		ident => Err(Error::new(ident.span(), "param_kind must be either `map` or `array`")),
	}
}
