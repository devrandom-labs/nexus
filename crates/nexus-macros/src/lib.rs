// TODO: create attribute macro for DomainEvent trait called nexus::
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

#[proc_macro_derive(Message)]
pub fn message(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let ident = ast.ident;
    quote! {
        impl ::nexus::core::Message for #ident {

        }
    }
    .into()
}

#[proc_macro_derive(Command, attributes(command))]
pub fn command(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let struct_name = &ast.ident;
    // TODO: get id type and result type, full type..
    // TODO: also add message trait to all command
    eprintln!("{:#?}", ast);
    TokenStream::new()
}
