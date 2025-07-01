#![allow(dead_code)]
use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, DeriveInput, Ident, Type, parse_macro_input};

mod utils;

#[proc_macro_derive(Command, attributes(command))]
pub fn command(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    eprintln!("{:#?}", &ast.attrs);

    let struct_name = &ast.ident;
    let command_attribute = parse_command_attributes(&ast.attrs);
    println!("{:?}", command_attribute);
    let _message_impl = impl_message(&struct_name);
    TokenStream::new()
}

#[derive(Debug)]
struct CommandAttribute {
    result: Type,
    error: Type,
}

fn parse_command_attributes(attributes: &[Attribute]) -> CommandAttribute {
    let command = attributes
        .iter()
        .find(|a| a.path().is_ident("command"))
        .expect("`#[command(...)]` attribute is required on the struct.");

    let mut result = None;
    let mut error = None;

    command
        .parse_nested_meta(|m| {
            if m.path.is_ident("result") {
                let value = m.value()?;
                result = Some(value.parse()?);
            } else if m.path.is_ident("error") {
                let value = m.value()?;
                error = Some(value.parse()?);
            } else {
                return Err(m.error("unrecognized command attribute"));
            }

            Ok(())
        })
        .unwrap();

    CommandAttribute {
        result: result.unwrap(),
        error: error.unwrap(),
    }
}

#[proc_macro_derive(Query, attributes(query))]
pub fn query(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let _struct_name = &ast.ident;
    eprintln!("{:#?}", ast);
    TokenStream::new()
}

#[proc_macro_derive(DomainEvent, attributes(domain_event))]
pub fn domain_event(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let _struct_name = &ast.ident;
    eprintln!("{:#?}", ast);
    TokenStream::new()
}

fn impl_message(ident: &Ident) -> TokenStream {
    quote! {
        impl ::nexus::core::Message for #ident {

        }
    }
    .into()
}

struct QueryAttribute {
    result: Type,
    error: Type,
}

struct DomainEventAttribute {
    id: Type,
}
