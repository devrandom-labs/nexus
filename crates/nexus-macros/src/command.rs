use super::utils::get_attribute;
use proc_macro2::TokenStream;
use syn::{DeriveInput, Result, Type};

#[derive(Debug)]
pub struct Command {
    result: Type,
    error: Type,
}

pub fn parse_command(ast: &DeriveInput) -> Result<TokenStream> {
    let name = &ast.ident;
    let attribute = get_attribute(&ast.attrs, "command", name.span())?;
    unimplemented!()
}
