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

/// Derive `nova_vm`'s `SoAble` for a struct with named fields: a struct of
/// arrays keeps one `Vec` per field (`<Name>Columns`), and hands out views of
/// one element (`<Name>Ref`, `<Name>Mut`) or of every element
/// (`<Name>Slice`, `<Name>SliceMut`) as structs of references, one per field.
///
/// The impl names `crate::heap::soa`, so the derive is for use inside `nova_vm`.
#[proc_macro_derive(SoAble)]
pub fn derive_soable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand_soable(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_soable(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new(
                    name.span(),
                    "#[derive(SoAble)] needs a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                name.span(),
                "#[derive(SoAble)] needs a struct with named fields",
            ));
        }
    };
    if fields.len() < 2 {
        return Err(syn::Error::new(
            name.span(),
            "#[derive(SoAble)] needs at least two fields; use a Vec instead",
        ));
    }
    if input.generics.type_params().next().is_some() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            input.generics.span(),
            "#[derive(SoAble)] supports lifetime parameters only",
        ));
    }
    let vis = &input.vis;
    let names: Vec<&Ident> = fields.iter().map(|f| f.ident.as_ref().unwrap()).collect();
    let first = names[0];
    let rest: Vec<&Ident> = names[1..].to_vec();
    let types: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();
    let field_vis: Vec<_> = fields.iter().map(|f| &f.vis).collect();
    let lifetimes: Vec<&Lifetime> = input.generics.lifetimes().map(|l| &l.lifetime).collect();

    let columns = format_ident!("{}Columns", name);
    let ref_name = format_ident!("{}Ref", name);
    let mut_name = format_ident!("{}Mut", name);
    let slice_name = format_ident!("{}Slice", name);
    let slice_mut_name = format_ident!("{}SliceMut", name);

    let own = quote! { <#(#lifetimes),*> };
    let helper = quote! { <'__soa, #(#lifetimes),*> };
    let helper_where = if lifetimes.is_empty() {
        quote! {}
    } else {
        quote! { where #(#lifetimes: '__soa),* }
    };

    Ok(quote! {
        #[allow(dead_code)]
        #vis struct #columns #own {
            #(#names: Vec<#types>),*
        }

        impl #own Default for #columns #own {
            #[inline]
            fn default() -> Self {
                Self { #(#names: Vec::new()),* }
            }
        }

        #[allow(dead_code)]
        #vis struct #ref_name #helper #helper_where {
            #(#field_vis #names: &'__soa #types),*
        }

        impl #helper Copy for #ref_name #helper #helper_where {}
        impl #helper Clone for #ref_name #helper #helper_where {
            #[inline(always)]
            fn clone(&self) -> Self {
                *self
            }
        }

        #[allow(dead_code)]
        #vis struct #mut_name #helper #helper_where {
            #(#field_vis #names: &'__soa mut #types),*
        }

        #[allow(dead_code)]
        #vis struct #slice_name #helper #helper_where {
            #(#field_vis #names: &'__soa [#types]),*
        }

        impl #helper Copy for #slice_name #helper #helper_where {}
        impl #helper Clone for #slice_name #helper #helper_where {
            #[inline(always)]
            fn clone(&self) -> Self {
                *self
            }
        }

        #[allow(dead_code)]
        #vis struct #slice_mut_name #helper #helper_where {
            #(#field_vis #names: &'__soa mut [#types]),*
        }

        impl #own crate::heap::soa::SoAble for #name #own {
            type Columns = #columns #own;
            type Ref<'__soa> = #ref_name<'__soa, #(#lifetimes),*> where Self: '__soa;
            type Mut<'__soa> = #mut_name<'__soa, #(#lifetimes),*> where Self: '__soa;
            type Slice<'__soa> = #slice_name<'__soa, #(#lifetimes),*> where Self: '__soa;
            type SliceMut<'__soa> = #slice_mut_name<'__soa, #(#lifetimes),*> where Self: '__soa;

            #[inline]
            fn with_capacity(__capacity: usize) -> Self::Columns {
                #columns { #(#names: Vec::with_capacity(__capacity)),* }
            }

            #[inline]
            fn reserve(__columns: &mut Self::Columns, __additional: usize) {
                #(__columns.#names.reserve(__additional);)*
            }

            #[inline]
            fn capacity(__columns: &Self::Columns) -> usize {
                __columns.#first.capacity()#(.min(__columns.#rest.capacity()))*
            }

            #[inline]
            fn push(__columns: &mut Self::Columns, __value: Self) {
                let Self { #(#names),* } = __value;
                #(__columns.#names.push(#names);)*
            }

            #[inline]
            fn get(__columns: &Self::Columns, __index: usize) -> Option<Self::Ref<'_>> {
                Some(#ref_name { #(#names: __columns.#names.get(__index)?),* })
            }

            #[inline]
            fn get_mut(__columns: &mut Self::Columns, __index: usize) -> Option<Self::Mut<'_>> {
                let #columns { #(#names),* } = __columns;
                Some(#mut_name { #(#names: #names.get_mut(__index)?),* })
            }

            #[inline]
            fn as_slice(__columns: &Self::Columns) -> Self::Slice<'_> {
                #slice_name { #(#names: __columns.#names.as_slice()),* }
            }

            #[inline]
            fn as_mut_slice(__columns: &mut Self::Columns) -> Self::SliceMut<'_> {
                let #columns { #(#names),* } = __columns;
                #slice_mut_name { #(#names: #names.as_mut_slice()),* }
            }

            #[inline]
            fn swap(__columns: &mut Self::Columns, __a: usize, __b: usize) {
                #(__columns.#names.swap(__a, __b);)*
            }

            #[inline]
            fn truncate(__columns: &mut Self::Columns, __len: usize) {
                #(__columns.#names.truncate(__len);)*
            }

            #[inline]
            fn pop(__columns: &mut Self::Columns) -> Option<Self> {
                #(let #names = __columns.#names.pop()?;)*
                Some(Self { #(#names),* })
            }
        }
    })
}
