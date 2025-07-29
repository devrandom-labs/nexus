use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Error, LitStr, Result, Type, parse_macro_input, spanned::Spanned};

mod utils;

#[proc_macro_derive(Command, attributes(command))]
pub fn command(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match parse_command(&ast) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}

#[proc_macro_derive(Query, attributes(query))]
pub fn query(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match parse_query(&ast) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}

#[proc_macro_derive(DomainEvent, attributes(domain_event, attribute_id))]
pub fn domain_event(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match parse_domain_event(&ast) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}

fn parse_command(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let attribute = utils::get_attribute(&ast.attrs, "command", name.span())?;

    let mut result: Option<Type> = None;
    let mut error_type: Option<Type> = None;

    attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident("result") {
            result = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("error") {
            error_type = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("unrecognized key for `#[command]` attribute"));
        }
        Ok(())
    })?;

    let result =
        result.ok_or_else(|| Error::new(attribute.path().span(), "`result` key is required"))?;

    let error_type =
        error_type.ok_or_else(|| Error::new(attribute.path().span(), "`error` key is required"))?;

    let expanded = quote! {
        impl ::nexus::domain::Command for #name {
            type Result = #result;
            type Error = #error_type;

        }

        impl ::nexus::domain::Message for #name {

        }
    };

    Ok(expanded)
}

fn parse_query(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let attribute = utils::get_attribute(&ast.attrs, "query", name.span())?;
    let mut result: Option<Type> = None;
    let mut error_type: Option<Type> = None;

    attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident("result") {
            result = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("error") {
            error_type = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("unrecognized key for `#[query]` attribute"));
        }
        Ok(())
    })?;

    let result =
        result.ok_or_else(|| Error::new(attribute.path().span(), "`result` key is required"))?;

    let error_type =
        error_type.ok_or_else(|| Error::new(attribute.path().span(), "`error` key is required"))?;

    let expanded = quote! {
        impl ::nexus::domain::Message for #name {

        }
        impl ::nexus::domain::Query for #name {
            type Result = #result;
            type Error = #error_type;

        }


    };

    Ok(expanded)
}

fn parse_domain_event(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    match &ast.data {
        Data::Struct(_) => {
            let attribute = utils::get_attribute(&ast.attrs, "domain_event", name.span())?;

            let mut event_name: Option<LitStr> = None;
            attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    event_name = Some(meta.value()?.parse()?);
                } else {
                    return Err(meta.error("unrecognized key for `#[domain_event]` attribute"));
                }

                Ok(())
            })?;

            let event_name = event_name
                .ok_or_else(|| Error::new(attribute.path().span(), "`name` key is required"))?;

            let expanded = quote! {
                impl ::nexus::domain::Message for #name {

                }

                impl ::nexus::domain::DomainEvent for #name {
                    fn name(&self) -> &'static str {
                        #event_name
                    }
                }
            };

            Ok(expanded)
        }
        Data::Enum(e) => Err(Error::new(name.span(), "Enums are not supported.")),
        Data::Union(_) => Err(Error::new(name.span(), "Unions are not supported.")),
    }
}
