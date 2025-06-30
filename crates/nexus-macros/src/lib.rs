// TODO: create derive macro for message message trait
// TODO: create attribute macro for DomainEvent trait called nexus::

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(Message)]
pub fn message(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    eprintln!("{:#?}", ast);
    TokenStream::new()
}
