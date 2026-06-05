extern crate proc_macro;

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, Data, DeriveInput, Fields, LitBool, Token};

#[derive(Clone, Debug)]
enum TagValue {
    Text(String),
    Numeric(u64),
}

/// Parses the `#[noun(tagged = bool)]` attribute from a list of attributes.
///
/// Used to determine whether an enum should be encoded with tags.
/// When applied to an enum, this attribute controls whether variant tags are included
/// in the noun representation.
///
/// # Arguments
///
/// * `attrs` - A slice of attributes to search through
///
/// # Returns
///
/// * `Some(bool)` - If the `tagged` attribute is found with a boolean value
/// * `None` - If the attribute is not found or has an invalid format
///
/// # Example
///
/// ```rust,ignore
/// #[derive(NounEncode, NounDecode)]
/// #[noun(tagged = false)]
/// enum MyEnum {
///     // variants...
/// }
/// ```rust,ignore
///
/// Tagged noun encoding: `[%variant [%variant1 value1] [%variant2 value2] ...]`
///
/// Untagged noun encoding: `[%variant value1 value2 ...]`
///
fn parse_noun_bool_attr(attrs: &[Attribute], key: &str) -> Option<bool> {
    let mut value = None;
    for attr in attrs {
        if !attr.path().is_ident("noun") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident(key) {
                if meta.input.peek(Token![=]) {
                    let lit = meta.value()?.parse::<LitBool>()?;
                    value = Some(lit.value());
                } else {
                    value = Some(true);
                }
            }
            Ok(())
        });
    }
    value
}

fn parse_tagged_attr(attrs: &[Attribute]) -> Option<bool> {
    parse_noun_bool_attr(attrs, "tagged")
}

fn parse_untagged_attr(attrs: &[Attribute]) -> Option<bool> {
    parse_noun_bool_attr(attrs, "untagged")
}

fn parse_tag_attr(attrs: &[Attribute]) -> Option<TagValue> {
    let mut value = None;
    for attr in attrs {
        if !attr.path().is_ident("noun") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                let parser = meta.value()?;
                let expr = parser.parse::<syn::Expr>()?;
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = expr
                {
                    value = Some(TagValue::Text(s.value()));
                } else if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(i),
                    ..
                }) = expr
                {
                    if let Ok(parsed) = i.base10_parse::<u64>() {
                        value = Some(TagValue::Numeric(parsed));
                    }
                }
            }
            Ok(())
        });
    }
    value
}

fn text_tag_atom_value(tag: &str) -> Option<u64> {
    let bytes = tag.as_bytes();
    if bytes.len() > 8 {
        return None;
    }

    let mut atom_bytes = [0u8; 8];
    atom_bytes[..bytes.len()].copy_from_slice(bytes);
    Some(u64::from_le_bytes(atom_bytes))
}

fn tag_value_matches_key(tag: &TagValue) -> String {
    match tag {
        TagValue::Text(value) => text_tag_atom_value(value)
            .map(|atom| format!("a:{atom}"))
            .unwrap_or_else(|| format!("s:{value}")),
        TagValue::Numeric(value) => format!("a:{value}"),
    }
}

fn resolve_variant_tag(attrs: &[Attribute], variant_name: &proc_macro2::Ident) -> TagValue {
    parse_tag_attr(attrs).unwrap_or_else(|| TagValue::Text(variant_name.to_string().to_lowercase()))
}

fn encode_tag_expr(tag: &TagValue) -> TokenStream2 {
    match tag {
        TagValue::Text(tag) => quote! { ::nockvm::ext::make_tas(allocator, #tag).as_noun() },
        TagValue::Numeric(tag) => quote! { ::nockvm::noun::D(#tag) },
    }
}

fn decode_tag_match_expr(tag: &TagValue) -> TokenStream2 {
    match tag {
        TagValue::Text(tag) => quote! { string_tag.as_deref() == Some(#tag) },
        TagValue::Numeric(tag) => quote! {
            tag_noun
                .as_atom()
                .ok()
                .and_then(|atom| atom.as_u64().ok())
                == Some(#tag)
        },
    }
}

/// Parses the `#[noun(axis = u64)]` attribute from a list of attributes.
///
/// Used to specify the axis of a field in a struct or tuple.
///
/// # Arguments
///
/// * `attrs` - A slice of attributes to search through
///
/// # Returns
///
/// * `Some(u64)` - If the `axis` attribute is found with a u64 value
/// * `None` - If the attribute is not found or has an invalid format
///
/// # Example
///
/// ```rust,ignore
/// #[derive(NounEncode, NounDecode)]
/// struct MyStruct {
///     field1: u64,
///     #[noun(axis = 2)]
///     field2: u64,
/// }
/// ```
fn parse_axis_attr(attrs: &[Attribute]) -> Option<u64> {
    attrs.iter().find_map(|attr| {
        if attr.path().is_ident("noun") {
            attr.parse_args::<syn::MetaNameValue>().ok().and_then(|nv| {
                if nv.path.is_ident("axis") {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Int(n),
                        ..
                    }) = nv.value
                    {
                        n.base10_parse::<u64>().ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        } else {
            None
        }
    })
}

#[proc_macro_derive(NounEncode, attributes(noun))]
/// Derives the `NounEncode` trait implementation for a struct or enum.
///
/// This macro generates code to convert Rust data structures into Urbit nouns.
/// It supports various encoding strategies based on attributes.
///
/// # Supported Types
///
/// - Structs with named fields, unnamed fields (tuples), or unit structs
/// - Enums with variants containing named fields, unnamed fields, or unit variants
///
/// # Attributes
///
/// - `#[noun(tag = "string")]`: Specifies a textual tag for enum variants (defaults to lowercase variant name)
/// - `#[noun(tag = 1)]`: Specifies an atom tag for enum variants, matching Hoon `%1`
/// - `#[noun(tagged = bool)]`: Controls whether fields are tagged with their names (enum-level or variant-level)
/// - `#[noun(untagged)]`: Encode enum variants without tags and try variants in order when decoding
///
/// # Encoding Format
///
/// ## Structs
/// - Named/Unnamed fields: Encoded as a cell containing all field values
/// - Unit structs: Encoded as atom `0`
///
/// ## Enums
/// - Tagged variant with named fields: `[%tag [[%field1 value1] [%field2 value2] ...]]`
/// - Untagged variant with named fields: `[%tag [value1 value2 ...]]`
/// - Variant with single unnamed field: `[%tag value]`
/// - Variant with multiple unnamed fields: `[%tag [field1 [field2 [...]]]]`
/// - Unit variant: `%tag`
///
/// # Example
///
/// ```rust,ignore
/// use noun_serde_derive::NounEncode;
/// use nockvm::noun::{NounAllocator, Noun};
/// use nockvm::mem::NockStack;
///
/// #[derive(NounEncode)]
/// struct Point {
///     x: u64,
///     y: u64,
/// }
///
/// // When encoded: [42 43]
/// let point = Point { x: 42, y: 43 };
/// let mut allocator = NockStack::new(8 << 10 << 10, 0);
/// let noun = point.to_noun(&mut allocator);
///
/// #[derive(NounEncode)]
/// #[noun(tagged = true)]
/// struct TaggedPoint {
///     x: u64,
///     y: u64,
/// }
///
/// // When encoded: [[%x 42] [%y 43]]
/// let tagged_point = TaggedPoint { x: 42, y: 43 };
/// let mut allocator = NockStack::new(8 << 10 << 10, 0);
/// let noun = tagged_point.to_noun(&mut allocator);
///
/// #[derive(NounEncode)]
/// #[noun(tagged = false)]
/// enum Command {
///     #[noun(tag = "move")]
///     Move { point: Point },
///     Stop,
/// }
///
/// // When encoded: [%move [42 43]]
/// let cmd = Command::Move { point: Point { x: 42, y: 43 } };
/// let mut allocator = NockStack::new(8 << 10 << 10, 0);
/// let noun = cmd.to_noun(&mut allocator);
///
/// // When encoded: %stop
/// let stop = Command::Stop;
/// let mut allocator = NockStack::new(8 << 10 << 10, 0);
/// let noun = stop.to_noun(&mut allocator);
/// ```
pub fn derive_noun_encode(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let enum_tagged = parse_tagged_attr(&input.attrs);
    let enum_untagged = parse_untagged_attr(&input.attrs).unwrap_or(false);

    let encode_impl = match input.data {
        Data::Struct(data) => {
            let field_encoders = match data.fields {
                Fields::Named(fields) => {
                    let field_count = fields.named.len();
                    let mut field_infos: Vec<(u64, &syn::Field, usize)> = fields
                        .named
                        .iter()
                        .enumerate()
                        .map(|(i, field)| {
                            let default_axis = if i == 0 {
                                2
                            } else if i == field_count - 1 {
                                let mut axis = 2;
                                for _ in 1..i {
                                    axis = 2 * axis + 2;
                                }
                                axis + 1
                            } else {
                                let mut axis = 2;
                                for _ in 1..=i {
                                    axis = 2 * axis + 2;
                                }
                                axis
                            };
                            let axis = parse_axis_attr(&field.attrs).unwrap_or(default_axis);
                            (axis, field, i)
                        })
                        .collect();

                    let has_custom_axis = field_infos
                        .iter()
                        .any(|(_axis, field, _)| parse_axis_attr(&field.attrs).is_some());
                    if has_custom_axis {
                        field_infos.sort_by_key(|(axis, _field, _i)| *axis);
                        let mut axes = field_infos
                            .iter()
                            .map(|(axis, _, _)| *axis)
                            .collect::<Vec<_>>();
                        axes.sort();
                        if axes.windows(2).any(|w| w[0] == w[1]) {
                            return syn::Error::new_spanned(
                                &name, "duplicate #[noun(axis = ...)] values in struct fields",
                            )
                            .to_compile_error()
                            .into();
                        }
                    }

                    let field_encoders = field_infos.iter().enumerate().map(|(out_i, (_axis, field, _in_i))| {
                        let field_name = field.ident.as_ref().expect("named field must have ident");
                        let field_var = format_ident!("field_{}", out_i);
                        quote! {
                            let #field_var = ::noun_serde::NounEncode::to_noun(&self.#field_name, allocator);
                            encoded_fields.push(#field_var);
                        }
                    });

                    if fields.named.is_empty() {
                        quote! { ::nockvm::noun::D(0) }
                    } else if fields.named.len() == 1 {
                        // Single field: just return the field itself
                        let field_name = fields
                            .named
                            .first()
                            .expect("field must exist")
                            .ident
                            .as_ref()
                            .expect("named field must have ident");
                        quote! {
                            ::noun_serde::NounEncode::to_noun(&self.#field_name, allocator)
                        }
                    } else {
                        quote! {
                            let mut encoded_fields = Vec::new();
                            #(#field_encoders)*
                            // Fold field nouns into a right-branching tree: [f1 [f2 [... fn]]]
                            // Note: No terminating 0 for structs
                            let mut result = encoded_fields.pop().unwrap();
                            for noun in encoded_fields.into_iter().rev() {
                                result = ::nockvm::noun::T(allocator, &[noun, result]);
                            }
                            result
                        }
                    }
                }
                Fields::Unnamed(fields) => {
                    let field_count = fields.unnamed.len();
                    if field_count == 0 {
                        quote! { ::nockvm::noun::D(0) }
                    } else if field_count == 1 {
                        // Single field: just return the field itself
                        quote! {
                            ::noun_serde::NounEncode::to_noun(&self.0, allocator)
                        }
                    } else {
                        let mut field_infos: Vec<(u64, usize)> = fields
                            .unnamed
                            .iter()
                            .enumerate()
                            .map(|(i, field)| {
                                let default_axis = if i == 0 {
                                    2
                                } else if i == field_count - 1 {
                                    let mut axis = 2;
                                    for _ in 1..i {
                                        axis = 2 * axis + 2;
                                    }
                                    axis + 1
                                } else {
                                    let mut axis = 2;
                                    for _ in 1..=i {
                                        axis = 2 * axis + 2;
                                    }
                                    axis
                                };
                                let axis = parse_axis_attr(&field.attrs).unwrap_or(default_axis);
                                (axis, i)
                            })
                            .collect();

                        let has_custom_axis = field_infos
                            .iter()
                            .any(|(_axis, i)| parse_axis_attr(&fields.unnamed[*i].attrs).is_some());
                        if has_custom_axis {
                            field_infos.sort_by_key(|(axis, _i)| *axis);
                            let mut axes = field_infos
                                .iter()
                                .map(|(axis, _)| *axis)
                                .collect::<Vec<_>>();
                            axes.sort();
                            if axes.windows(2).any(|w| w[0] == w[1]) {
                                return syn::Error::new_spanned(
                                    &name, "duplicate #[noun(axis = ...)] values in tuple fields",
                                )
                                .to_compile_error()
                                .into();
                            }
                        }

                        let field_encoders = field_infos.iter().enumerate().map(|(out_i, (_axis, i))| {
                            let idx = syn::Index::from(*i);
                            let field_var = format_ident!("field_{}", out_i);
                            quote! {
                                let #field_var = ::noun_serde::NounEncode::to_noun(&self.#idx, allocator);
                                encoded_fields.push(#field_var);
                            }
                        });

                        quote! {
                            let mut encoded_fields = Vec::new();
                            #(#field_encoders)*
                            // Fold field nouns into a right-branching tree: [f1 [f2 [... fn]]]
                            // Note: No terminating 0 for structs
                            let mut result = encoded_fields.pop().unwrap();
                            for noun in encoded_fields.into_iter().rev() {
                                result = ::nockvm::noun::T(allocator, &[noun, result]);
                            }
                            result
                        }
                    }
                }
                Fields::Unit => {
                    quote! {
                        ::nockvm::noun::D(0)
                    }
                }
            };

            quote! {
                #field_encoders
            }
        }
        Data::Enum(data) => {
            if enum_untagged {
                let cases: Vec<_> = data
                    .variants
                    .iter()
                    .map(|variant| {
                        let variant_name = &variant.ident;
                        match &variant.fields {
                            Fields::Named(fields) => {
                                let field_names: Vec<_> = fields
                                    .named
                                    .iter()
                                    .map(|f| f.ident.as_ref().expect("named field must have ident"))
                                    .collect();

                                if fields.named.is_empty() {
                                    quote! {
                                        #name::#variant_name { } => {
                                            ::nockvm::noun::D(0)
                                        }
                                    }
                                } else if fields.named.len() == 1 {
                                    let field_name = field_names[0];
                                    quote! {
                                        #name::#variant_name { #field_name } => {
                                            ::noun_serde::NounEncode::to_noun(#field_name, allocator)
                                        }
                                    }
                                } else {
                                    let field_encoders = fields.named.iter().enumerate().map(|(i, field)| {
                                        let field_name = field.ident.as_ref().expect("named field must have ident");
                                        let field_var = format_ident!("encoded_field_{}", i);
                                        quote! {
                                            let #field_var = ::noun_serde::NounEncode::to_noun(#field_name, allocator);
                                            encoded_fields.push(#field_var);
                                        }
                                    });
                                    quote! {
                                        #name::#variant_name { #(#field_names),* } => {
                                            let mut encoded_fields = Vec::new();
                                            #(#field_encoders)*
                                            let mut result = encoded_fields.pop().unwrap();
                                            for noun in encoded_fields.into_iter().rev() {
                                                result = ::nockvm::noun::T(allocator, &[noun, result]);
                                            }
                                            result
                                        }
                                    }
                                }
                            }
                            Fields::Unnamed(fields) => {
                                let field_count = fields.unnamed.len();
                                let field_idents: Vec<_> = (0..field_count)
                                    .map(|i| format_ident!("field_{}", i))
                                    .collect();

                                if field_count == 0 {
                                    quote! {
                                        #name::#variant_name => {
                                            ::nockvm::noun::D(0)
                                        }
                                    }
                                } else if field_count == 1 {
                                    quote! {
                                        #name::#variant_name(value) => {
                                            ::noun_serde::NounEncode::to_noun(value, allocator)
                                        }
                                    }
                                } else {
                                    let field_encoders = field_idents.iter().enumerate().map(|(i, ident)| {
                                        let field_var = format_ident!("encoded_field_{}", i);
                                        quote! {
                                            let #field_var = ::noun_serde::NounEncode::to_noun(#ident, allocator);
                                            encoded_fields.push(#field_var);
                                        }
                                    });
                                    quote! {
                                        #name::#variant_name(#(#field_idents),*) => {
                                            let mut encoded_fields = Vec::new();
                                            #(#field_encoders)*
                                            let mut result = encoded_fields.pop().unwrap();
                                            for noun in encoded_fields.into_iter().rev() {
                                                result = ::nockvm::noun::T(allocator, &[noun, result]);
                                            }
                                            result
                                        }
                                    }
                                }
                            }
                            Fields::Unit => {
                                quote! {
                                    #name::#variant_name => {
                                        ::nockvm::noun::D(0)
                                    }
                                }
                            }
                        }
                    })
                    .collect();

                quote! {
                    match self {
                        #(#cases),*
                    }
                }
            } else {
                let mut seen_tags = HashSet::new();
                for variant in data.variants.iter() {
                    let variant_untagged = parse_untagged_attr(&variant.attrs).unwrap_or(false);
                    if variant_untagged {
                        continue;
                    }
                    let tag = resolve_variant_tag(&variant.attrs, &variant.ident);
                    let key = tag_value_matches_key(&tag);
                    if !seen_tags.insert(key) {
                        return syn::Error::new_spanned(
                            &variant.ident, "duplicate enum tag in #[derive(NounEncode)]",
                        )
                        .to_compile_error()
                        .into();
                    }
                }

                let mut cases = Vec::new();
                for variant in data.variants.iter() {
                    let variant_name = &variant.ident;
                    let tag = resolve_variant_tag(&variant.attrs, variant_name);
                    let tag_expr = encode_tag_expr(&tag);

                    // Check variant-level tagged attribute, fallback to enum-level
                    let is_tagged =
                        parse_tagged_attr(&variant.attrs).unwrap_or(enum_tagged.unwrap_or(false));
                    let variant_untagged = parse_untagged_attr(&variant.attrs).unwrap_or(false);

                    let case = match &variant.fields {
                        Fields::Named(fields) => {
                            let field_names: Vec<_> = fields
                                .named
                                .iter()
                                .map(|f| f.ident.as_ref().expect("named field must have ident"))
                                .collect();

                            if variant_untagged {
                                if fields.named.is_empty() {
                                    quote! {
                                        #name::#variant_name { } => {
                                            ::nockvm::noun::D(0)
                                        }
                                    }
                                } else if fields.named.len() == 1 {
                                    let field_name = field_names[0];
                                    quote! {
                                        #name::#variant_name { #field_name } => {
                                            ::noun_serde::NounEncode::to_noun(#field_name, allocator)
                                        }
                                    }
                                } else {
                                    let field_encoders = fields.named.iter().enumerate().map(|(i, field)| {
                                        let field_name = field.ident.as_ref().expect("named field must have ident");
                                        let field_var = format_ident!("encoded_field_{}", i);
                                        quote! {
                                            let #field_var = ::noun_serde::NounEncode::to_noun(#field_name, allocator);
                                            encoded_fields.push(#field_var);
                                        }
                                    });
                                    quote! {
                                        #name::#variant_name { #(#field_names),* } => {
                                            let mut encoded_fields = Vec::new();
                                            #(#field_encoders)*
                                            let mut result = encoded_fields.pop().unwrap();
                                            for noun in encoded_fields.into_iter().rev() {
                                                result = ::nockvm::noun::T(allocator, &[noun, result]);
                                            }
                                            result
                                        }
                                    }
                                }
                            } else if is_tagged {
                                quote! {
                                    #name::#variant_name { #(#field_names),* } => {
                                        let tag = #tag_expr;
                                        let mut field_nouns = Vec::new();
                                        #(
                                            let field_tag = ::nockvm::ext::make_tas(allocator, stringify!(#field_names)).as_noun();
                                            let field_value = ::noun_serde::NounEncode::to_noun(#field_names, allocator);
                                            field_nouns.push(::nockvm::noun::T(allocator, &[field_tag, field_value]));
                                        )*
                                        let data = field_nouns.into_iter().rev().fold(::nockvm::noun::D(0), |acc, pair_noun| {
                                             if acc.is_atom() && acc.as_atom().map_or(false, |a| a.as_u64() == Ok(0)) {
                                                ::nockvm::noun::T(allocator, &[pair_noun, ::nockvm::noun::D(0)])
                                            } else {
                                                ::nockvm::noun::T(allocator, &[pair_noun, acc])
                                            }
                                        });
                                        ::nockvm::noun::T(allocator, &[tag, data])
                                    }
                                }
                            } else {
                                quote! {
                                    #name::#variant_name { #(#field_names),* } => {
                                        let tag = #tag_expr;
                                        let mut field_nouns = vec![tag];
                                        #(
                                            let field_noun = ::noun_serde::NounEncode::to_noun(#field_names, allocator);
                                            field_nouns.push(field_noun);
                                        )*
                                        ::nockvm::noun::T(allocator, &field_nouns)
                                    }
                                }
                            }
                        }
                        Fields::Unnamed(fields) => {
                            let field_count = fields.unnamed.len();
                            let field_idents: Vec<_> = (0..field_count)
                                .map(|i| format_ident!("field_{}", i))
                                .collect();

                            if variant_untagged {
                                if field_count == 0 {
                                    quote! {
                                        #name::#variant_name => {
                                            ::nockvm::noun::D(0)
                                        }
                                    }
                                } else if field_count == 1 {
                                    quote! {
                                        #name::#variant_name(value) => {
                                            ::noun_serde::NounEncode::to_noun(value, allocator)
                                        }
                                    }
                                } else {
                                    let field_encoders = field_idents.iter().enumerate().map(|(i, ident)| {
                                        let field_var = format_ident!("encoded_field_{}", i);
                                        quote! {
                                            let #field_var = ::noun_serde::NounEncode::to_noun(#ident, allocator);
                                            encoded_fields.push(#field_var);
                                        }
                                    });
                                    quote! {
                                        #name::#variant_name(#(#field_idents),*) => {
                                            let mut encoded_fields = Vec::new();
                                            #(#field_encoders)*
                                            let mut result = encoded_fields.pop().unwrap();
                                            for noun in encoded_fields.into_iter().rev() {
                                                result = ::nockvm::noun::T(allocator, &[noun, result]);
                                            }
                                            result
                                        }
                                    }
                                }
                            } else if field_count == 1 {
                                quote! {
                                    #name::#variant_name(value) => {
                                        let tag = #tag_expr;
                                        let data = ::noun_serde::NounEncode::to_noun(value, allocator);
                                        ::nockvm::noun::T(allocator, &[tag, data])
                                    }
                                }
                            } else {
                                let field_idents_rev =
                                    field_idents.iter().rev().collect::<Vec<_>>();
                                let first_field = field_idents_rev[0];
                                let rest_fields = &field_idents_rev[1..];

                                quote! {
                                    #name::#variant_name(#(#field_idents),*) => {
                                        let tag = #tag_expr;
                                        let mut data = ::noun_serde::NounEncode::to_noun(#first_field, allocator);
                                        #(
                                            let next = ::noun_serde::NounEncode::to_noun(#rest_fields, allocator);
                                            data = ::nockvm::noun::T(allocator, &[next, data]);
                                        )*
                                        ::nockvm::noun::T(allocator, &[tag, data])
                                    }
                                }
                            }
                        }
                        Fields::Unit => {
                            if variant_untagged {
                                quote! {
                                    #name::#variant_name => {
                                        ::nockvm::noun::D(0)
                                    }
                                }
                            } else {
                                quote! {
                                    #name::#variant_name => {
                                        #tag_expr
                                    }
                                }
                            }
                        }
                    };
                    cases.push(case);
                }

                quote! {
                    match self {
                        #(#cases),*
                    }
                }
            }
        }
        Data::Union(_) => {
            panic!("Union types are not supported by NounEncode");
        }
    };

    // Generate the impl block
    let expanded = quote! {
        impl ::noun_serde::NounEncode for #name {
            fn to_noun<A: ::nockvm::noun::NounAllocator>(&self, allocator: &mut A) -> ::nockvm::noun::Noun {
                #encode_impl
            }
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(NounDecode, attributes(noun))]
pub fn derive_noun_decode(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident.clone();
    let name_str = name.to_string();

    // Get enum-level tagged attribute
    let enum_tagged = parse_tagged_attr(&input.attrs);
    let enum_untagged = parse_untagged_attr(&input.attrs).unwrap_or(false);

    // Generate implementation based on the type
    let decode_impl = match input.data {
        Data::Struct(data) => {
            match data.fields {
                Fields::Named(fields) => {
                    let field_names: Vec<_> = fields
                        .named
                        .iter()
                        .map(|f| f.ident.as_ref().expect("named field must have ident"))
                        .collect();

                    let field_types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();

                    if fields.named.is_empty() {
                        quote! {
                            ::tracing::trace!(target: "noun_serde_decode", "Decoding {} (empty struct)", #name_str);
                            Ok(Self {})
                        }
                    } else if fields.named.len() == 1 {
                        // Single field: decode directly from noun
                        let field_name = &field_names[0];
                        let field_name_str = field_name.to_string();
                        let field_type = &field_types[0];
                        quote! {
                            ::tracing::trace!(target: "noun_serde_decode", "Decoding {} (single field struct), is_atom={}, is_cell={}", #name_str, noun.is_atom(), noun.is_cell());
                            ::tracing::trace!(target: "noun_serde_decode", "  field={} type={}", #field_name_str, stringify!(#field_type));
                            let #field_name = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)
                                .map_err(|e| {
                                    ::tracing::trace!(target: "noun_serde_decode", "  FAILED decoding field {} in {}: {:?}", #field_name_str, #name_str, e);
                                    e
                                })?;
                            ::tracing::trace!(target: "noun_serde_decode", "  SUCCESS decoded field {} in {}", #field_name_str, #name_str);
                            Ok(Self { #field_name })
                        }
                    } else {
                        let num_fields = fields.named.len();
                        // Generate field decoding using correct tree addressing with optional custom axis
                        let field_decoders = field_names
                            .iter()
                            .zip(field_types.iter())
                            .enumerate()
                            .map(|(i, (name, ty))| {
                                // Get the corresponding field
                                let field = fields
                                    .named
                                    .iter()
                                    .find(|f| f.ident.as_ref().expect("named field must have ident") == *name)
                                    .expect("field must exist");

                                // Check for custom axis
                                let custom_axis = parse_axis_attr(&field.attrs);

                                // Calculate the axis for right-branching binary tree
                                // Pattern:
                                // - First field: axis 2
                                // - Middle fields: 2 * previous_axis + 2
                                // - Last field: previous_axis + 1
                                // Examples:
                                // - 2 fields [x y]: x=2, y=3 (2+1)
                                // - 3 fields [x [y z]]: x=2, y=6 (2*2+2), z=7 (6+1)
                                // - 4 fields [x [y [z w]]]: x=2, y=6 (2*2+2), z=14 (2*6+2), w=15 (14+1)
                                let default_axis = if i == 0 {
                                    2  // first field always at axis 2
                                } else if i == num_fields - 1 {
                                    // Last field: previous_axis + 1
                                    let mut axis = 2;
                                    for _ in 1..i {
                                        axis = 2 * axis + 2;
                                    }
                                    axis + 1
                                } else {
                                    // Middle fields: 2 * previous_axis + 2
                                    let mut axis = 2;
                                    for _ in 1..=i {
                                        axis = 2 * axis + 2;
                                    }
                                    axis
                                };

                                let axis = custom_axis.unwrap_or(default_axis);
                                let field_name_str = name.to_string();
                                quote! {
                                    ::tracing::trace!(target: "noun_serde_decode", "  field={} type={} axis={}", #field_name_str, stringify!(#ty), #axis);
                                    let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                        .map_err(|e| {
                                            ::tracing::trace!(target: "noun_serde_decode", "  FAILED to get slot {} for field {} in {}: {:?}", #axis, #field_name_str, #name_str, e);
                                            ::noun_serde::NounDecodeError::ExpectedCell
                                        })?;
                                    ::tracing::trace!(target: "noun_serde_decode", "  field={} is_atom={} is_cell={}", #field_name_str, field_noun.is_atom(), field_noun.is_cell());
                                    let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)
                                        .map_err(|e| {
                                            ::tracing::trace!(target: "noun_serde_decode", "  FAILED decoding field {} in {}: {:?}", #field_name_str, #name_str, e);
                                            e
                                        })?;
                                    ::tracing::trace!(target: "noun_serde_decode", "  SUCCESS decoded field {} in {}", #field_name_str, #name_str);
                                }
                            });

                        quote! {
                            ::tracing::trace!(target: "noun_serde_decode", "Decoding {} (multi-field struct), is_atom={}, is_cell={}", #name_str, noun.is_atom(), noun.is_cell());
                            let cell = noun.as_cell().map_err(|e| {
                                ::tracing::trace!(target: "noun_serde_decode", "FAILED {} expected cell but got atom", #name_str);
                                ::noun_serde::NounDecodeError::ExpectedCell
                            })?;
                            #(#field_decoders)*
                            ::tracing::trace!(target: "noun_serde_decode", "SUCCESS decoded {}", #name_str);
                            Ok(Self {
                                #(#field_names),*
                            })
                        }
                    }
                }
                Fields::Unnamed(fields) => {
                    let field_count = fields.unnamed.len() as u64;
                    if field_count == 1 {
                        let field_type = &fields.unnamed[0].ty;
                        quote! {
                            ::tracing::trace!(target: "noun_serde_decode", "Decoding {} (single field tuple), is_atom={}, is_cell={}", #name_str, noun.is_atom(), noun.is_cell());
                            ::tracing::trace!(target: "noun_serde_decode", "  field=0 type={}", stringify!(#field_type));
                            let field_0 = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)
                                .map_err(|e| {
                                    ::tracing::trace!(target: "noun_serde_decode", "  FAILED decoding field 0 in {}: {:?}", #name_str, e);
                                    e
                                })?;
                            ::tracing::trace!(target: "noun_serde_decode", "SUCCESS decoded {}", #name_str);
                            Ok(Self(field_0))
                        }
                    } else {
                        let field_decoders = (0..field_count).map(|i| {
                            let field_ident = format_ident!("field_{}", i);
                            let field_type = &fields.unnamed[i as usize].ty;

                            // Check if there's a custom axis specified in the field attributes
                            let field = &fields.unnamed[i as usize];
                            let custom_axis = field.attrs.iter()
                                .find_map(|attr| {
                                    if attr.path().is_ident("noun") {
                                        attr.parse_args::<syn::MetaNameValue>().ok()
                                            .and_then(|nv| if nv.path.is_ident("axis") {
                                                if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(n), .. }) = nv.value {
                                                    n.base10_parse::<u64>().ok()
                                                } else {
                                                    None
                                                }
                                            } else {
                                                None
                                            })
                                    } else {
                                        None
                                    }
                                });

                            // Calculate the axis for right-branching binary tree
                            // Pattern:
                            // - First field: axis 2
                            // - Middle fields: 2 * previous_axis + 2
                            // - Last field: previous_axis + 1
                            // Examples:
                            // - 2 fields [x y]: x=2, y=3 (2+1)
                            // - 3 fields [x [y z]]: x=2, y=6 (2*2+2), z=7 (6+1)
                            // - 4 fields [x [y [z w]]]: x=2, y=6 (2*2+2), z=14 (2*6+2), w=15 (14+1)
                            let default_axis = if i == 0 {
                                2  // first field always at axis 2
                            } else if i == field_count - 1 {
                                // Last field: previous_axis + 1
                                let mut axis = 2;
                                for _ in 1..i {
                                    axis = 2 * axis + 2;
                                }
                                axis + 1
                            } else {
                                // Middle fields: 2 * previous_axis + 2
                                let mut axis = 2;
                                for _ in 1..=i {
                                    axis = 2 * axis + 2;
                                }
                                axis
                            };

                            let axis = custom_axis.unwrap_or(default_axis);
                            let field_num_str = i.to_string();

                            quote! {
                                ::tracing::trace!(target: "noun_serde_decode", "  field={} type={} axis={}", #field_num_str, stringify!(#field_type), #axis);
                                let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                    .map_err(|e| {
                                        ::tracing::trace!(target: "noun_serde_decode", "  FAILED to get slot {} for field {} in {}: {:?}", #axis, #field_num_str, #name_str, e);
                                        ::noun_serde::NounDecodeError::FieldError(stringify!(#field_ident).to_string(), "Missing field".into())
                                    })?;
                                ::tracing::trace!(target: "noun_serde_decode", "  field={} is_atom={} is_cell={}", #field_num_str, field_noun.is_atom(), field_noun.is_cell());
                                let #field_ident = <#field_type as ::noun_serde::NounDecode>::from_noun(&field_noun)
                                    .map_err(|e| {
                                        ::tracing::trace!(target: "noun_serde_decode", "  FAILED decoding field {} in {}: {:?}", #field_num_str, #name_str, e);
                                        ::noun_serde::NounDecodeError::FieldError(stringify!(#field_ident).to_string(), e.to_string())
                                    })?;
                                ::tracing::trace!(target: "noun_serde_decode", "  SUCCESS decoded field {} in {}", #field_num_str, #name_str);
                            }
                        });

                        let field_idents = (0..field_count).map(|i| format_ident!("field_{}", i));

                        quote! {
                            ::tracing::trace!(target: "noun_serde_decode", "Decoding {} (multi-field tuple), is_atom={}, is_cell={}", #name_str, noun.is_atom(), noun.is_cell());
                            let cell = noun.as_cell().map_err(|e| {
                                ::tracing::trace!(target: "noun_serde_decode", "FAILED {} expected cell but got atom", #name_str);
                                ::noun_serde::NounDecodeError::ExpectedCell
                            })?;
                            #(#field_decoders)*
                            ::tracing::trace!(target: "noun_serde_decode", "SUCCESS decoded {}", #name_str);
                            Ok(Self(#(#field_idents),*))
                        }
                    }
                }
                Fields::Unit => {
                    quote! {
                        ::tracing::trace!(target: "noun_serde_decode", "Decoding {} (unit struct)", #name_str);
                        ::tracing::trace!(target: "noun_serde_decode", "SUCCESS decoded {}", #name_str);
                        Ok(Self)
                    }
                }
            }
        }
        Data::Enum(data) => {
            if enum_untagged {
                let attempts: Vec<_> = data
                    .variants
                    .iter()
                    .map(|variant| {
                        let variant_name = &variant.ident;
                        let attempt = match &variant.fields {
                            Fields::Named(fields) => {
                                let field_names: Vec<_> = fields
                                    .named
                                    .iter()
                                    .map(|f| f.ident.as_ref().expect("named field must have ident"))
                                    .collect();
                                let field_types: Vec<_> =
                                    fields.named.iter().map(|f| &f.ty).collect();

                                if fields.named.is_empty() {
                                    quote! {
                                        let atom = noun
                                            .as_atom()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedAtom)?;
                                        let atom_u64 = atom
                                            .as_u64()
                                            .map_err(|_| ::noun_serde::NounDecodeError::InvalidEnumVariant)?;
                                        if atom_u64 == 0 {
                                            Ok(Self::#variant_name { })
                                        } else {
                                            Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                                        }
                                    }
                                } else if fields.named.len() == 1 {
                                    let field_name = field_names[0];
                                    let field_type = field_types[0];
                                    quote! {
                                        let #field_name = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)?;
                                        Ok(Self::#variant_name { #field_name })
                                    }
                                } else {
                                    let num_fields = field_names.len();
                                    let field_decoders = field_names
                                        .iter()
                                        .zip(field_types.iter())
                                        .enumerate()
                                        .map(|(i, (name, ty))| {
                                            let field = fields
                                                .named
                                                .iter()
                                                .find(|f| {
                                                    f.ident
                                                        .as_ref()
                                                        .expect("named field must have ident")
                                                        == *name
                                                })
                                                .expect("field must exist");
                                            let custom_axis = parse_axis_attr(&field.attrs);
                                            let default_axis = if i == 0 {
                                                2
                                            } else if i == num_fields - 1 {
                                                let mut axis = 2;
                                                for _ in 1..i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis + 1
                                            } else {
                                                let mut axis = 2;
                                                for _ in 1..=i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis
                                            };
                                            let axis = custom_axis.unwrap_or(default_axis);
                                            quote! {
                                                let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                                    .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                                let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)?;
                                            }
                                        });
                                    quote! {
                                        let cell = noun
                                            .as_cell()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                        #(#field_decoders)*
                                        Ok(Self::#variant_name { #(#field_names),* })
                                    }
                                }
                            }
                            Fields::Unnamed(fields) => {
                                let field_count = fields.unnamed.len();
                                let field_names: Vec<_> = (0..field_count)
                                    .map(|i| format_ident!("field_{}", i))
                                    .collect();
                                let field_types: Vec<_> =
                                    fields.unnamed.iter().map(|f| &f.ty).collect();

                                if field_count == 0 {
                                    quote! {
                                        let atom = noun
                                            .as_atom()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedAtom)?;
                                        let atom_u64 = atom
                                            .as_u64()
                                            .map_err(|_| ::noun_serde::NounDecodeError::InvalidEnumVariant)?;
                                        if atom_u64 == 0 {
                                            Ok(Self::#variant_name)
                                        } else {
                                            Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                                        }
                                    }
                                } else if field_count == 1 {
                                    let field_type = field_types[0];
                                    quote! {
                                        let value = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)?;
                                        Ok(Self::#variant_name(value))
                                    }
                                } else {
                                    let field_decoders = field_names
                                        .iter()
                                        .zip(field_types.iter())
                                        .enumerate()
                                        .map(|(i, (name, ty))| {
                                            let field = &fields.unnamed[i];
                                            let custom_axis = parse_axis_attr(&field.attrs);
                                            let default_axis = if i == 0 {
                                                2
                                            } else if i == field_count - 1 {
                                                let mut axis = 2;
                                                for _ in 1..i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis + 1
                                            } else {
                                                let mut axis = 2;
                                                for _ in 1..=i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis
                                            };
                                            let axis = custom_axis.unwrap_or(default_axis);
                                            quote! {
                                                let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                                    .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                                let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)?;
                                            }
                                        });
                                    quote! {
                                        let cell = noun
                                            .as_cell()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                        #(#field_decoders)*
                                        Ok(Self::#variant_name(#(#field_names),*))
                                    }
                                }
                            }
                            Fields::Unit => {
                                quote! {
                                    let atom = noun
                                        .as_atom()
                                        .map_err(|_| ::noun_serde::NounDecodeError::ExpectedAtom)?;
                                    let atom_u64 = atom
                                        .as_u64()
                                        .map_err(|_| ::noun_serde::NounDecodeError::InvalidEnumVariant)?;
                                    if atom_u64 == 0 {
                                        Ok(Self::#variant_name)
                                    } else {
                                        Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                                    }
                                }
                            }
                        };

                        quote! {
                            match (|| -> Result<Self, ::noun_serde::NounDecodeError> {
                                #attempt
                            })() {
                                Ok(value) => return Ok(value),
                                Err(err) => last_err = Some(err),
                            }
                        }
                    })
                    .collect();

                quote! {
                    ::tracing::trace!(target: "noun_serde_decode", "Decoding enum {} (untagged), is_atom={}, is_cell={}", #name_str, noun.is_atom(), noun.is_cell());
                    let mut last_err: Option<::noun_serde::NounDecodeError> = None;
                    #(#attempts)*
                    Err(last_err.unwrap_or(::noun_serde::NounDecodeError::InvalidEnumVariant))
                }
            } else {
                let mut seen_tags = HashSet::new();
                for variant in data.variants.iter() {
                    let variant_untagged = parse_untagged_attr(&variant.attrs).unwrap_or(false);
                    if variant_untagged {
                        continue;
                    }
                    let tag = resolve_variant_tag(&variant.attrs, &variant.ident);
                    let key = tag_value_matches_key(&tag);
                    if !seen_tags.insert(key) {
                        return syn::Error::new_spanned(
                            &variant.ident, "duplicate enum tag in #[derive(NounDecode)]",
                        )
                        .to_compile_error()
                        .into();
                    }
                }

                let untagged_attempts: Vec<_> = data
                    .variants
                    .iter()
                    .filter(|variant| parse_untagged_attr(&variant.attrs).unwrap_or(false))
                    .map(|variant| {
                        let variant_name = &variant.ident;
                        let attempt = match &variant.fields {
                            Fields::Named(fields) => {
                                let field_names: Vec<_> = fields
                                    .named
                                    .iter()
                                    .map(|f| f.ident.as_ref().expect("named field must have ident"))
                                    .collect();
                                let field_types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();

                                if field_names.is_empty() {
                                    quote! {
                                        let atom = noun
                                            .as_atom()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedAtom)?;
                                        let atom_u64 = atom
                                            .as_u64()
                                            .map_err(|_| ::noun_serde::NounDecodeError::InvalidEnumVariant)?;
                                        if atom_u64 == 0 {
                                            Ok(Self::#variant_name { })
                                        } else {
                                            Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                                        }
                                    }
                                } else if field_names.len() == 1 {
                                    let field_name = field_names[0];
                                    let field_type = field_types[0];
                                    quote! {
                                        let #field_name = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)?;
                                        Ok(Self::#variant_name { #field_name })
                                    }
                                } else {
                                    let num_fields = field_names.len();
                                    let field_decoders = field_names
                                        .iter()
                                        .zip(field_types.iter())
                                        .enumerate()
                                        .map(|(i, (name, ty))| {
                                            let field = fields
                                                .named
                                                .iter()
                                                .find(|f| {
                                                    f.ident
                                                        .as_ref()
                                                        .expect("named field must have ident")
                                                        == *name
                                                })
                                                .expect("field must exist");
                                            let custom_axis = parse_axis_attr(&field.attrs);
                                            let default_axis = if i == 0 {
                                                2
                                            } else if i == num_fields - 1 {
                                                let mut axis = 2;
                                                for _ in 1..i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis + 1
                                            } else {
                                                let mut axis = 2;
                                                for _ in 1..=i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis
                                            };
                                            let axis = custom_axis.unwrap_or(default_axis);
                                            quote! {
                                                let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                                    .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                                let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)?;
                                            }
                                        });
                                    quote! {
                                        let cell = noun
                                            .as_cell()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                        #(#field_decoders)*
                                        Ok(Self::#variant_name { #(#field_names),* })
                                    }
                                }
                            }
                            Fields::Unnamed(fields) => {
                                let field_count = fields.unnamed.len();
                                let field_names: Vec<_> = (0..field_count)
                                    .map(|i| format_ident!("field_{}", i))
                                    .collect();
                                let field_types: Vec<_> =
                                    fields.unnamed.iter().map(|f| &f.ty).collect();

                                if field_count == 0 {
                                    quote! {
                                        let atom = noun
                                            .as_atom()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedAtom)?;
                                        let atom_u64 = atom
                                            .as_u64()
                                            .map_err(|_| ::noun_serde::NounDecodeError::InvalidEnumVariant)?;
                                        if atom_u64 == 0 {
                                            Ok(Self::#variant_name)
                                        } else {
                                            Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                                        }
                                    }
                                } else if field_count == 1 {
                                    let field_type = field_types[0];
                                    quote! {
                                        let value = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)?;
                                        Ok(Self::#variant_name(value))
                                    }
                                } else {
                                    let field_decoders = field_names
                                        .iter()
                                        .zip(field_types.iter())
                                        .enumerate()
                                        .map(|(i, (name, ty))| {
                                            let field = &fields.unnamed[i];
                                            let custom_axis = parse_axis_attr(&field.attrs);
                                            let default_axis = if i == 0 {
                                                2
                                            } else if i == field_count - 1 {
                                                let mut axis = 2;
                                                for _ in 1..i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis + 1
                                            } else {
                                                let mut axis = 2;
                                                for _ in 1..=i {
                                                    axis = 2 * axis + 2;
                                                }
                                                axis
                                            };
                                            let axis = custom_axis.unwrap_or(default_axis);
                                            quote! {
                                                let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                                    .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                                let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)?;
                                            }
                                        });
                                    quote! {
                                        let cell = noun
                                            .as_cell()
                                            .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                        #(#field_decoders)*
                                        Ok(Self::#variant_name(#(#field_names),*))
                                    }
                                }
                            }
                            Fields::Unit => {
                                quote! {
                                    let atom = noun
                                        .as_atom()
                                        .map_err(|_| ::noun_serde::NounDecodeError::ExpectedAtom)?;
                                    let atom_u64 = atom
                                        .as_u64()
                                        .map_err(|_| ::noun_serde::NounDecodeError::InvalidEnumVariant)?;
                                    if atom_u64 == 0 {
                                        Ok(Self::#variant_name)
                                    } else {
                                        Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                                    }
                                }
                            }
                        };
                        quote! {
                            if let Ok(value) = (|| -> Result<Self, ::noun_serde::NounDecodeError> {
                                #attempt
                            })() {
                                return Ok(value);
                            }
                        }
                    })
                    .collect();

                let mut cases = Vec::new();
                for variant in data
                    .variants
                    .iter()
                    .filter(|variant| !parse_untagged_attr(&variant.attrs).unwrap_or(false))
                {
                    let variant_name = &variant.ident;
                    let tag = resolve_variant_tag(&variant.attrs, variant_name);
                    let tag_match_expr = decode_tag_match_expr(&tag);

                    let is_tagged =
                        parse_tagged_attr(&variant.attrs).unwrap_or(enum_tagged.unwrap_or(false));

                    let case = match &variant.fields {
                        Fields::Named(fields) => {
                            let field_names: Vec<_> = fields
                                .named
                                .iter()
                                .map(|f| f.ident.as_ref().expect("named field must have ident"))
                                .collect();
                            let field_types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();

                            if is_tagged {
                                let field_decoders = field_names
                                    .iter()
                                    .zip(field_types.iter())
                                    .enumerate()
                                    .map(|(i, (name, ty))| {
                                        let field = fields
                                            .named
                                            .iter()
                                            .find(|f| {
                                                f.ident
                                                    .as_ref()
                                                    .expect("named field must have ident")
                                                    == *name
                                            })
                                            .expect("field must exist");
                                        let custom_axis = parse_axis_attr(&field.attrs);
                                        let default_axis = if i == 0 {
                                            2
                                        } else {
                                            let mut axis = 1;
                                            for _ in 0..i {
                                                axis = axis * 2 + 1;
                                            }
                                            axis * 2
                                        };
                                        let axis = custom_axis.unwrap_or(default_axis);
                                        quote! {
                                            let field_cell = ::nockvm::noun::Slots::slot(&data, #axis)
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?
                                                .as_cell()
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_cell.tail())?;
                                        }
                                    });

                                quote! {
                                    if #tag_match_expr {
                                        return (|| -> Result<Self, ::noun_serde::NounDecodeError> {
                                            let cell = noun
                                                .as_cell()
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let data = cell.tail();
                                            #(#field_decoders)*
                                            Ok(Self::#variant_name { #(#field_names),* })
                                        })();
                                    }
                                }
                            } else {
                                let num_fields = field_names.len();
                                let field_decoders = field_names
                                    .iter()
                                    .zip(field_types.iter())
                                    .enumerate()
                                    .map(|(i, (name, ty))| {
                                        let field = fields
                                            .named
                                            .iter()
                                            .find(|f| {
                                                f.ident
                                                    .as_ref()
                                                    .expect("named field must have ident")
                                                    == *name
                                            })
                                            .expect("field must exist");
                                        let custom_axis = parse_axis_attr(&field.attrs);
                                        let default_axis = if i == 0 {
                                            2
                                        } else if i == num_fields - 1 {
                                            let mut axis = 2;
                                            for _ in 1..i {
                                                axis = 2 * axis + 2;
                                            }
                                            axis + 1
                                        } else {
                                            let mut axis = 2;
                                            for _ in 1..=i {
                                                axis = 2 * axis + 2;
                                            }
                                            axis
                                        };
                                        let axis = custom_axis.unwrap_or(default_axis);
                                        quote! {
                                            let field_noun = ::nockvm::noun::Slots::slot(&data_cell, #axis)
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)?;
                                        }
                                    });

                                let payload_atom_handler = if num_fields == 1 {
                                    let field_name = field_names[0];
                                    let field_type = field_types[0];
                                    quote! {
                                        let #field_name = <#field_type as ::noun_serde::NounDecode>::from_noun(&payload)?;
                                        Ok(Self::#variant_name { #field_name })
                                    }
                                } else {
                                    quote! {
                                        Err(::noun_serde::NounDecodeError::ExpectedCell)
                                    }
                                };

                                quote! {
                                    if #tag_match_expr {
                                        return (|| -> Result<Self, ::noun_serde::NounDecodeError> {
                                            let cell = noun
                                                .as_cell()
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let payload = cell.tail();
                                            if let Ok(payload_cell) = payload.as_cell() {
                                                let data_cell = payload_cell;
                                                #(#field_decoders)*
                                                Ok(Self::#variant_name { #(#field_names),* })
                                            } else {
                                                #payload_atom_handler
                                            }
                                        })();
                                    }
                                }
                            }
                        }
                        Fields::Unnamed(fields) => {
                            let field_count = fields.unnamed.len();
                            let field_names: Vec<_> = (0..field_count)
                                .map(|i| format_ident!("field_{}", i))
                                .collect();
                            let field_types: Vec<_> =
                                fields.unnamed.iter().map(|f| &f.ty).collect();

                            if field_count == 0 {
                                quote! {
                                    if #tag_match_expr {
                                        return Ok(Self::#variant_name);
                                    }
                                }
                            } else if field_count == 1 {
                                let ty = field_types[0];
                                quote! {
                                    if #tag_match_expr {
                                        return (|| -> Result<Self, ::noun_serde::NounDecodeError> {
                                            let cell = noun
                                                .as_cell()
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let value = <#ty as ::noun_serde::NounDecode>::from_noun(&cell.tail())?;
                                            Ok(Self::#variant_name(value))
                                        })();
                                    }
                                }
                            } else {
                                let field_decoders = field_names
                                    .iter()
                                    .zip(field_types.iter())
                                    .enumerate()
                                    .map(|(i, (name, ty))| {
                                        let field = &fields.unnamed[i];
                                        let custom_axis = parse_axis_attr(&field.attrs);
                                        let default_axis = if i == 0 {
                                            2
                                        } else if i == field_count - 1 {
                                            let mut axis = 2;
                                            for _ in 1..i {
                                                axis = 2 * axis + 2;
                                            }
                                            axis + 1
                                        } else {
                                            let mut axis = 2;
                                            for _ in 1..=i {
                                                axis = 2 * axis + 2;
                                            }
                                            axis
                                        };
                                        let axis = custom_axis.unwrap_or(default_axis);
                                        quote! {
                                            let field_noun = ::nockvm::noun::Slots::slot(&data_cell, #axis)
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_noun)?;
                                        }
                                    });

                                quote! {
                                    if #tag_match_expr {
                                        return (|| -> Result<Self, ::noun_serde::NounDecodeError> {
                                            let cell = noun
                                                .as_cell()
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            let data_cell = cell
                                                .tail()
                                                .as_cell()
                                                .map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                                            #(#field_decoders)*
                                            Ok(Self::#variant_name(#(#field_names),*))
                                        })();
                                    }
                                }
                            }
                        }
                        Fields::Unit => {
                            quote! {
                                if #tag_match_expr {
                                    return Ok(Self::#variant_name);
                                }
                            }
                        }
                    };
                    cases.push(case);
                }

                quote! {
                    #(#untagged_attempts)*

                    let tag_noun = if noun.is_atom() {
                        *noun
                    } else if let Ok(cell) = noun.as_cell() {
                        cell.head()
                    } else {
                        return Err(::noun_serde::NounDecodeError::InvalidEnumData);
                    };
                    let string_tag = tag_noun
                        .as_atom()
                        .ok()
                        .and_then(|atom| {
                            ::std::str::from_utf8(atom.as_ne_bytes())
                                .ok()
                                .map(|tag| tag.trim_end_matches('\0').to_string())
                        });

                    #(#cases)*
                    Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
                }
            }
        }
        Data::Union(_) => {
            panic!("Union types are not supported by NounDecode");
        }
    };

    // Generate the impl block
    let expanded = quote! {
        impl ::noun_serde::NounDecode for #name {
            fn from_noun(noun: &::nockvm::noun::Noun) -> Result<Self, ::noun_serde::NounDecodeError> {
                #decode_impl
            }
        }
    };

    TokenStream::from(expanded)
}
