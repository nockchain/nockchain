extern crate proc_macro;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, Data, DeriveInput, Fields};

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
fn parse_tagged_attr(attrs: &[Attribute]) -> Option<bool> {
    attrs.iter().find_map(|attr| {
        if attr.path().is_ident("noun") {
            attr.parse_args::<syn::MetaNameValue>().ok().and_then(|nv| {
                if nv.path.is_ident("tagged") {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Bool(b),
                        ..
                    }) = nv.value
                    {
                        Some(b.value())
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
/// - `#[noun(tag = "string")]`: Specifies a custom tag for enum variants (defaults to lowercase variant name)
/// - `#[noun(tagged = bool)]`: Controls whether fields are tagged with their names (enum-level or variant-level)
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

    let encode_impl = match input.data {
        Data::Struct(data) => {
            let field_encoders = match data.fields {
                Fields::Named(fields) => {
                    let field_encoders = fields.named.iter().enumerate().map(|(i, field)| {
                        let field_name = field.ident.as_ref().unwrap();
                        let field_var = format_ident!("field_{}", i);
                        quote! {
                            let #field_var = ::noun_serde::NounEncode::to_noun(&self.#field_name, allocator);
                            encoded_fields.push(#field_var);
                        }
                    });

                    if fields.named.is_empty() {
                        quote! { ::nockvm::noun::D(0) }
                    } else if fields.named.len() == 1 {
                        // Single field: just return the field itself
                        let field_name = fields.named.first().unwrap().ident.as_ref().unwrap();
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
                        let field_encoders = (0..field_count).map(|i| {
                            let idx = syn::Index::from(i);
                            let field_var = format_ident!("field_{}", i);
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
            let cases: Vec<_> = data
                .variants
                .iter()
                .map(|variant| {
                    let variant_name = &variant.ident;
                    let tag = variant
                        .attrs
                        .iter()
                        .find_map(|attr| {
                            if attr.path().is_ident("noun") {
                                attr.parse_args::<syn::MetaNameValue>().ok().and_then(|nv| {
                                    if nv.path.is_ident("tag") {
                                        if let syn::Expr::Lit(syn::ExprLit {
                                            lit: syn::Lit::Str(s),
                                            ..
                                        }) = nv.value
                                        {
                                            Some(s.value())
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
                        .unwrap_or_else(|| variant_name.to_string().to_lowercase());

                    // Check variant-level tagged attribute, fallback to enum-level
                    let is_tagged = parse_tagged_attr(&variant.attrs).unwrap_or(enum_tagged.unwrap_or(false));

                    match &variant.fields {
                        Fields::Named(fields) => {
                            let field_names: Vec<_> = fields
                                .named
                                .iter()
                                .map(|f| f.ident.as_ref().unwrap())
                                .collect();

                            if is_tagged {
                                // Tagged encoding: [%tag [[%field1 value1] [%field2 value2] ...]]
                                quote! {
                                    #name::#variant_name { #(#field_names),* } => {
                                        let tag = ::nockapp::utils::make_tas(allocator, #tag).as_noun();
                                        let mut field_nouns = Vec::new();
                                        #(
                                            let field_tag = ::nockapp::utils::make_tas(allocator, stringify!(#field_names)).as_noun();
                                            let field_value = ::noun_serde::NounEncode::to_noun(#field_names, allocator);
                                            field_nouns.push(::nockvm::noun::T(allocator, &[field_tag, field_value]));
                                        )*
                                        // Fold field pairs into a list: [[k1 v1] [[k2 v2] ... 0]]
                                        let data = field_nouns.into_iter().rev().fold(::nockvm::noun::D(0), |acc, pair_noun| {
                                             if acc.is_atom() && acc.as_atom().map_or(false, |a| a.as_u64() == Ok(0)) {
                                                ::nockvm::noun::T(allocator, &[pair_noun, ::nockvm::noun::D(0)]) // Base case: [last_pair 0]
                                            } else {
                                                ::nockvm::noun::T(allocator, &[pair_noun, acc])
                                            }
                                        });
                                        ::nockvm::noun::T(allocator, &[tag, data])
                                    }
                                }
                            } else {
                                // Untagged encoding: [%tag [value1 value2 ...]]
                                quote! {
                                    #name::#variant_name { #(#field_names),* } => {
                                        let tag = ::nockapp::utils::make_tas(allocator, #tag).as_noun();
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

                            if field_count == 1 {
                                let _ty = &fields.unnamed[0].ty;
                                quote! {
                                    #name::#variant_name(value) => {
                                        let tag = ::nockapp::utils::make_tas(allocator, #tag).as_noun();
                                        let data = ::noun_serde::NounEncode::to_noun(value, allocator);
                                        ::nockvm::noun::T(allocator, &[tag, data])
                                    }
                                }
                            } else {
                                let field_idents_rev = field_idents.iter().rev().collect::<Vec<_>>();
                                let first_field = field_idents_rev[0];
                                let rest_fields = &field_idents_rev[1..];

                                quote! {
                                    #name::#variant_name(#(#field_idents),*) => {
                                        let tag = ::nockapp::utils::make_tas(allocator, #tag).as_noun();
                                        // Build nested cell structure right-to-left
                                        let mut data = ::noun_serde::NounEncode::to_noun(#first_field, allocator);
                                        #(
                                            data = ::nockvm::noun::T(allocator, &[::noun_serde::NounEncode::to_noun(#rest_fields, allocator), data]);
                                        )*
                                        ::nockvm::noun::T(allocator, &[tag, data])
                                    }
                                }
                            }
                        }
                        Fields::Unit => {
                            quote! {
                                #name::#variant_name => {
                                    ::nockapp::utils::make_tas(allocator, #tag).as_noun()
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
    let name = input.ident;

    // Get enum-level tagged attribute
    let enum_tagged = parse_tagged_attr(&input.attrs);

    // Generate implementation based on the type
    let decode_impl = match input.data {
        Data::Struct(data) => {
            match data.fields {
                Fields::Named(fields) => {
                    let field_names: Vec<_> = fields
                        .named
                        .iter()
                        .map(|f| f.ident.as_ref().unwrap())
                        .collect();

                    let field_types: Vec<_> = fields.named.iter().map(|f| &f.ty).collect();

                    if fields.named.is_empty() {
                        quote! {
                            Ok(Self {})
                        }
                    } else if fields.named.len() == 1 {
                        // Single field: decode directly from noun
                        let field_name = &field_names[0];
                        let field_type = &field_types[0];
                        quote! {
                            let #field_name = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)?;
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
                                    .find(|f| f.ident.as_ref().unwrap().to_string() == name.to_string())
                                    .unwrap();

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
                                quote! {
                                    let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&::nockvm::noun::Slots::slot(&cell, #axis)?)?;
                                }
                            });

                        quote! {
                            let cell = noun.as_cell().map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                            #(#field_decoders)*
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
                            let field_0 = <#field_type as ::noun_serde::NounDecode>::from_noun(noun)?;
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

                            quote! {
                                let field_noun = ::nockvm::noun::Slots::slot(&cell, #axis)
                                    .map_err(|_| ::noun_serde::NounDecodeError::FieldError(stringify!(#field_ident).to_string(), "Missing field".into()))?;
                                let #field_ident = <#field_type as ::noun_serde::NounDecode>::from_noun(&field_noun)
                                    .map_err(|e| ::noun_serde::NounDecodeError::FieldError(stringify!(#field_ident).to_string(), e.to_string()))?;
                            }
                        });

                        let field_idents = (0..field_count).map(|i| format_ident!("field_{}", i));

                        quote! {
                            let cell = noun.as_cell().map_err(|_| ::noun_serde::NounDecodeError::ExpectedCell)?;
                            #(#field_decoders)*
                            Ok(Self(#(#field_idents),*))
                        }
                    }
                }
                Fields::Unit => {
                    quote! {
                        Ok(Self)
                    }
                }
            }
        }
        Data::Enum(data) => {
            let cases: Vec<_> = data.variants.iter().map(|variant| {
                let variant_name = &variant.ident;
                let tag = variant.attrs.iter()
                    .find_map(|attr| {
                        if attr.path().is_ident("noun") {
                            attr.parse_args::<syn::MetaNameValue>().ok()
                                .and_then(|nv| if nv.path.is_ident("tag") {
                                    if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = nv.value {
                                        Some(s.value())
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                })
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| variant_name.to_string().to_lowercase());

                // Check variant-level tagged attribute, fallback to enum-level
                let is_tagged = parse_tagged_attr(&variant.attrs).unwrap_or(enum_tagged.unwrap_or(false));

                match &variant.fields {
                    Fields::Named(fields) => {
                        let field_names: Vec<_> = fields.named.iter()
                            .map(|f| f.ident.as_ref().unwrap())
                            .collect();

                        let field_types: Vec<_> = fields.named.iter()
                            .map(|f| &f.ty)
                            .collect();

                        if is_tagged {
                            // Tagged decoding: [%tag [[%field1 value1] [%field2 value2] ...]]
                            let field_decoders = field_names.iter().zip(field_types.iter()).enumerate()
                                .map(|(i, (name, ty))| {
                                    // print name and type decoding
                                    // Get the corresponding field
                                    let field = fields.named.iter().find(|f| {
                                        f.ident.as_ref().unwrap().to_string() == name.to_string()
                                    }).unwrap();

                                    // Check for custom axis
                                    let custom_axis = parse_axis_attr(&field.attrs);

                                    // Calculate the axis for right-branching binary tree
                                    // For field i, axis = 2 for i=0, axis = ((1*2+1)...*2+1)*2 for i>0
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
                                        let field_cell = ::nockvm::noun::Slots::slot(&data, #axis)?.as_cell()?;
                                        let #name = <#ty as ::noun_serde::NounDecode>::from_noun(&field_cell.tail())?;
                                    }
                                });

                            quote! {
                                tag if tag == #tag => {
                                    if let Ok(cell) = noun.as_cell() {
                                        let data = cell.tail();
                                        #(#field_decoders)*
                                        Ok(Self::#variant_name { #(#field_names),* })
                                    } else {
                                        Err(::noun_serde::NounDecodeError::ExpectedCell)
                                    }
                                }
                            }
                        } else {
                            // Untagged decoding: [%tag value1 value2 ...]
                            quote! {
                                tag if tag == #tag => {
                                    if let Ok(cell) = noun.as_cell() {
                                        let tail = cell.tail();
                                        let tail_cell = tail.as_cell()?;
                                        #(
                                            let #field_names = <#field_types as ::noun_serde::NounDecode>::from_noun(
                                                &if stringify!(#field_names) == stringify!(x) {
                                                    tail_cell.head()
                                                } else {
                                                    tail_cell.tail()
                                                }
                                            )?;
                                        )*
                                        Ok(Self::#variant_name { #(#field_names),* })
                                    } else {
                                        Err(::noun_serde::NounDecodeError::ExpectedCell)
                                    }
                                }
                            }
                        }
                    }
                    Fields::Unnamed(fields) => {
                        let field_count = fields.unnamed.len();
                        let field_names: Vec<_> = (0..field_count)
                            .map(|i| format_ident!("field_{}", i))
                            .collect();

                        let field_types: Vec<_> = fields.unnamed.iter()
                            .map(|f| &f.ty)
                            .collect();

                        if field_count == 1 {
                            let ty = &field_types[0];
                            quote! {
                                tag if tag == #tag => {
                                    if let Ok(cell) = noun.as_cell() {
                                        let value = <#ty as ::noun_serde::NounDecode>::from_noun(&cell.tail())?;
                                        Ok(Self::#variant_name(value))
                                    } else {
                                        Err(::noun_serde::NounDecodeError::ExpectedCell)
                                    }
                                }
                            }
                        } else {
                            let field_idents_rev = field_names.iter().rev().collect::<Vec<_>>();
                            let first_field = field_idents_rev[0];
                            let rest_fields = &field_idents_rev[1..];

                            quote! {
                                #name::#variant_name(#(#field_names),*) => {
                                    let tag = ::nockapp::utils::make_tas(allocator, #tag).as_noun();
                                    // Build nested cell structure right-to-left
                                    let mut data = ::noun_serde::NounEncode::to_noun(#first_field, allocator);
                                    #(
                                        data = ::nockvm::noun::T(allocator, &[::noun_serde::NounEncode::to_noun(#rest_fields, allocator), data]);
                                    )*
                                    ::nockvm::noun::T(allocator, &[tag, data])
                                }
                            }
                        }
                    }
                    Fields::Unit => {
                        quote! {
                            tag if tag == #tag => Ok(Self::#variant_name)
                        }
                    }
                }
            }).collect();

            quote! {
                let tag = if let Ok(atom) = noun.as_atom() {
                    let bytes = atom.as_ne_bytes();
                    ::std::str::from_utf8(bytes)
                        .map_err(|_| ::noun_serde::NounDecodeError::InvalidTag)?
                        .trim_end_matches('\0')
                        .to_string()
                } else if let Ok(cell) = noun.as_cell() {
                    let atom = cell.head().as_atom()
                        .map_err(|_| ::noun_serde::NounDecodeError::InvalidTag)?;
                    let bytes = atom.as_ne_bytes();
                    ::std::str::from_utf8(bytes)
                        .map_err(|_| ::noun_serde::NounDecodeError::InvalidTag)?
                        .trim_end_matches('\0')
                        .to_string()
                } else {
                    return Err(::noun_serde::NounDecodeError::InvalidEnumData);
                };

                match tag.as_str() {
                    #(#cases,)*
                    _ => Err(::noun_serde::NounDecodeError::InvalidEnumVariant)
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
