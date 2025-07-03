#![allow(dead_code)]
use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Result, Type, parse_macro_input, spanned::Spanned};

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

#[proc_macro_derive(DomainEvent, attributes(domain_event))]
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
        impl ::nexus::core::Command for #name {
            type Result = #result;
            type Error = #error_type;

        }

        impl ::nexus::core::Message for #name {

        }
    }
    .into();

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
        impl ::nexus::core::Message for #name {

        }
        impl ::nexus::core::Query for #name {
            type Result = #result;
            type Error = #error_type;

        }


    }
    .into();

    Ok(expanded)
}

fn parse_domain_event(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let attribute = utils::get_attribute(&ast.attrs, "domain_event", name.span())?;
    // this is a bit diff, we need ID
    //
    let mut id: Option<Type> = None;

    attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("unrecognized key for `#[command]` attribute"));
        }
        Ok(())
    })?;

    let id = id.ok_or_else(|| Error::new(attribute.path().span(), "`id` key is required"))?;

    let expanded = quote! {

        impl ::nexus::core::Message for #name {

        }

        impl ::nexus::core::DomainEvent for #name {
            type Id = #id;
        }

    }
    .into();

    Ok(expanded)
}
