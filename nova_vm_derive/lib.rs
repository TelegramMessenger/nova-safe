// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Derive macros for `nova_vm`.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2, TokenTree};
use quote::{format_ident, quote};
use syn::{
    Attribute, Data, DeriveInput, Fields, GenericParam, Lifetime, Type, parse_macro_input,
    spanned::Spanned,
};

/// Derive `nova_vm`'s `Bindable` for a type with exactly one lifetime
/// parameter, the garbage collector lifetime, and no other generics.
///
/// `bind` and `unbind` rebuild the value: an exhaustive `match` over every
/// variant of an enum, or a full struct literal. A field whose type mentions
/// the lifetime is converted with `Bindable::bind` / `Bindable::unbind`
/// (`PhantomData` fields are recreated); any other field is moved as is. The
/// compiler checks the result against `Self::Of<'gc>`, so a field that is
/// missed or converted wrongly is a compile error rather than a wrong handle.
///
/// The impl names `crate::engine`, so the derive is for use inside `nova_vm`.
#[proc_macro_derive(Bindable)]
pub fn derive_bindable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

#[derive(Clone, Copy)]
enum Direction {
    Unbind,
    Bind,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let mut params = input.generics.params.iter();
    let lifetime = match (params.next(), params.next()) {
        (Some(GenericParam::Lifetime(param)), None) => param.lifetime.clone(),
        _ => {
            return Err(syn::Error::new(
                input.generics.span(),
                "#[derive(Bindable)] needs exactly one lifetime parameter and no other generics",
            ));
        }
    };
    if input.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            input.generics.span(),
            "#[derive(Bindable)] does not support where clauses",
        ));
    }
    let unbind_body = body(input, &lifetime, Direction::Unbind)?;
    let bind_body = body(input, &lifetime, Direction::Bind)?;
    Ok(quote! {
        impl crate::engine::Bindable for #name<'_> {
            type Of<'__gc> = #name<'__gc>;

            #[inline(always)]
            fn unbind(self) -> Self::Of<'static> {
                #unbind_body
            }

            #[inline(always)]
            fn bind<'__gc>(
                self,
                __gc: crate::engine::NoGcScope<'__gc, '_>,
            ) -> Self::Of<'__gc> {
                #bind_body
            }
        }
    })
}

fn body(
    input: &DeriveInput,
    lifetime: &Lifetime,
    direction: Direction,
) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    match &input.data {
        Data::Struct(data) => {
            let (pattern, construct) = fields(&quote!(#name), &data.fields, lifetime, direction)?;
            Ok(quote! {
                let #pattern = self;
                #construct
            })
        }
        Data::Enum(data) => {
            let mut arms = Vec::new();
            for variant in &data.variants {
                let variant_name = &variant.ident;
                let cfgs = cfg_attributes(&variant.attrs);
                let (pattern, construct) = fields(
                    &quote!(#name::#variant_name),
                    &variant.fields,
                    lifetime,
                    direction,
                )?;
                arms.push(quote! {
                    #(#cfgs)*
                    #pattern => #construct,
                });
            }
            Ok(quote! {
                match self {
                    #(#arms)*
                }
            })
        }
        Data::Union(_) => Err(syn::Error::new(
            name.span(),
            "#[derive(Bindable)] does not support unions",
        )),
    }
}

/// The destructuring pattern and the rebuilt value for one struct or variant.
fn fields(
    path: &TokenStream2,
    fields: &Fields,
    lifetime: &Lifetime,
    direction: Direction,
) -> syn::Result<(TokenStream2, TokenStream2)> {
    Ok(match fields {
        Fields::Unit => (quote!(#path), quote!(#path)),
        Fields::Unnamed(unnamed) => {
            let mut patterns = Vec::new();
            let mut values = Vec::new();
            for (index, field) in unnamed.unnamed.iter().enumerate() {
                let binding = format_ident!("__field_{}", index);
                let cfgs = cfg_attributes(&field.attrs);
                let value = convert(&binding, &field.ty, lifetime, direction)?;
                patterns.push(quote!(#(#cfgs)* #binding));
                values.push(quote!(#(#cfgs)* #value));
            }
            (quote!(#path(#(#patterns),*)), quote!(#path(#(#values),*)))
        }
        Fields::Named(named) => {
            let mut patterns = Vec::new();
            let mut values = Vec::new();
            for (index, field) in named.named.iter().enumerate() {
                let field_name = field.ident.as_ref().expect("named field");
                let binding = format_ident!("__field_{}", index);
                let cfgs = cfg_attributes(&field.attrs);
                let value = convert(&binding, &field.ty, lifetime, direction)?;
                patterns.push(quote!(#(#cfgs)* #field_name: #binding));
                values.push(quote!(#(#cfgs)* #field_name: #value));
            }
            (
                quote!(#path { #(#patterns),* }),
                quote!(#path { #(#values),* }),
            )
        }
    })
}

fn convert(
    binding: &Ident,
    ty: &Type,
    lifetime: &Lifetime,
    direction: Direction,
) -> syn::Result<TokenStream2> {
    if is_phantom_data(ty) {
        return Ok(quote!(core::marker::PhantomData));
    }
    if !mentions_lifetime(quote!(#ty), &lifetime.ident) {
        return Ok(quote!(#binding));
    }
    if let Some(container) = allocating_container(ty) {
        return Err(syn::Error::new(
            ty.span(),
            format!(
                "#[derive(Bindable)] would rebuild this `{container}` on every bind and unbind; \
                 store its items unbound or keep a hand-written impl"
            ),
        ));
    }
    Ok(match direction {
        Direction::Unbind => quote!(crate::engine::Bindable::unbind(#binding)),
        Direction::Bind => quote!(crate::engine::Bindable::bind(#binding, __gc)),
    })
}

/// A container whose `Bindable` impl reallocates or walks its items, found
/// anywhere inside `ty`.
fn allocating_container(ty: &Type) -> Option<String> {
    const CONTAINERS: &[&str] = &[
        "Vec",
        "VecDeque",
        "Box",
        "Rc",
        "Arc",
        "HashMap",
        "HashSet",
        "AHashMap",
        "AHashSet",
        "BTreeMap",
        "BTreeSet",
        "HashTable",
    ];
    fn find(tokens: TokenStream2) -> Option<String> {
        for token in tokens {
            match token {
                TokenTree::Ident(ident) => {
                    let name = ident.to_string();
                    if CONTAINERS.contains(&name.as_str()) {
                        return Some(name);
                    }
                }
                TokenTree::Group(group) => {
                    if let Some(found) = find(group.stream()) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    find(quote!(#ty))
}

fn is_phantom_data(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "PhantomData"),
        _ => false,
    }
}

fn mentions_lifetime(tokens: TokenStream2, lifetime: &Ident) -> bool {
    let mut after_quote = false;
    for token in tokens {
        match token {
            TokenTree::Punct(punct) => after_quote = punct.as_char() == '\'',
            TokenTree::Ident(ident) => {
                if after_quote && ident == *lifetime {
                    return true;
                }
                after_quote = false;
            }
            TokenTree::Group(group) => {
                if mentions_lifetime(group.stream(), lifetime) {
                    return true;
                }
                after_quote = false;
            }
            TokenTree::Literal(_) => after_quote = false,
        }
    }
    false
}

fn cfg_attributes(attributes: &[Attribute]) -> Vec<&Attribute> {
    attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("cfg"))
        .collect()
}
