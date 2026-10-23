//! Compile localized formatting into separately formatted arguments. Interpolated
//! paths/names are never parsed, translated or used as translation keys.
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, LitStr, Token, parse::Parse, parse::ParseStream, parse_macro_input};

struct Message {
    source: LitStr,
    args: Vec<Expr>,
}
impl Parse for Message {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let source = input.parse()?;
        let mut args = Vec::new();
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            args.push(input.parse()?);
        }
        Ok(Self { source, args })
    }
}

#[proc_macro]
pub fn trf(input: TokenStream) -> TokenStream {
    let message = parse_macro_input!(input as Message);
    expand(message)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
fn expand(message: Message) -> syn::Result<proc_macro2::TokenStream> {
    let mut bindings = Vec::new();
    let mut positional = Vec::new();
    let mut named = std::collections::HashMap::new();
    for (index, expr) in message.args.iter().enumerate() {
        let binding = format_ident!("__re_i18n_arg_{index}");
        let value = if let Expr::Assign(assign) = expr {
            let Expr::Path(path) = &*assign.left else {
                return Err(syn::Error::new_spanned(
                    expr,
                    "expected a named format argument",
                ));
            };
            let Some(name) = path.path.get_ident() else {
                return Err(syn::Error::new_spanned(path, "expected an argument name"));
            };
            named.insert(name.to_string(), binding.clone());
            &*assign.right
        } else {
            positional.push(binding.clone());
            expr
        };
        bindings.push(quote!(let #binding = &(#value);));
    }
    let source = &message.source;
    let text = source.value();
    let mut index = 0;
    let mut implicit = 0;
    let mut values = Vec::new();
    while index < text.len() {
        let tail = &text[index..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            index += 2;
            continue;
        }
        if tail.starts_with('{') {
            let Some(end) = tail.find('}') else {
                return Err(syn::Error::new_spanned(
                    source,
                    "unclosed localized placeholder",
                ));
            };
            let field = &tail[1..end];
            let (name, spec) = field.split_once(':').unwrap_or((field, ""));
            let resolved = if name.is_empty() {
                let n = implicit;
                implicit += 1;
                n.to_string()
            } else {
                name.to_string()
            };
            let value = if let Ok(position) = resolved.parse::<usize>() {
                let Some(binding) = positional.get(position) else {
                    return Err(syn::Error::new_spanned(
                        source,
                        "missing localized format argument",
                    ));
                };
                quote!(#binding)
            } else if let Some(binding) = named.get(&resolved) {
                quote!(#binding)
            } else {
                let ident: syn::Ident = syn::parse_str(&resolved)
                    .map_err(|_| syn::Error::new_spanned(source, "invalid localized capture"))?;
                quote!(&#ident)
            };
            let key = if spec.is_empty() {
                resolved
            } else {
                format!("{resolved}:{spec}")
            };
            let key = LitStr::new(&key, source.span());
            let formatter = LitStr::new(&format!("{{:{spec}}}"), source.span());
            values.push(quote!((#key, ::std::format!(#formatter, #value))));
            index += end + 1;
        } else {
            index += tail.chars().next().unwrap().len_utf8();
        }
    }
    Ok(quote!({
        #(#bindings)*
        ::rust_explorer::i18n::render(#source, &[#(#values),*])
    }))
}
