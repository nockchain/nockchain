use std::mem::size_of;

use proc_macro::TokenStream;
use quote::quote;
use syn::{self, LitByteStr};

#[proc_macro]
pub fn tas(input: TokenStream) -> TokenStream {
    let byte_str: LitByteStr = syn::parse(input).expect("failed to parse input");
    let bytes = byte_str.value();
    if bytes.len() > size_of::<u64>() {
        panic!(
            "\"{}\" does not fit in a u64: must be 8 or fewer characters, not {}",
            byte_str.token(),
            bytes.len()
        );
    }
    let mut val: u64 = 0;
    for byte in bytes.into_iter().rev() {
        val = (val << u8::BITS) | u64::from(byte);
    }
    quote!(#val).into()
}
