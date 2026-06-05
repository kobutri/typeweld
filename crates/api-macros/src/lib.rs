//! Proc macro placeholders.

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn api(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
