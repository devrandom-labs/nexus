#![allow(dead_code)]
use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, DeriveInput, Ident, Result, Type, parse_macro_input};

mod command;
mod utils;

#[proc_macro_derive(Command, attributes(command))]
pub fn command(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    eprintln!("{:#?}", &ast.ident);
    TokenStream::new()
}
