use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, LitStr, Result, Type, parse_macro_input, spanned::Spanned};
use utils::DataTypesFieldInfo;

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

#[proc_macro_derive(Aggregate, attributes(aggregate))]
pub fn aggregate(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match parse_aggregate(&ast) {
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
        impl ::nexus::core::Message for #name {

        }
        impl ::nexus::core::Query for #name {
            type Result = #result;
            type Error = #error_type;

        }


    };

    Ok(expanded)
}

fn parse_domain_event(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    match utils::get_fields_info(&ast.data, "attribute_id", ast.span())? {
        DataTypesFieldInfo::Struct {
            name: field_name,
            ty: id_type,
        } => {
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
                impl ::nexus::core::Message for #name {

                }

                impl ::nexus::core::DomainEvent for #name {
                    type Id = #id_type;

                    fn aggregate_id(&self) -> &Self::Id {
                        &self.#field_name
                    }

                    fn name(&self) -> &'static str {
                        #event_name
                    }
                }
            };

            Ok(expanded)
        }
        DataTypesFieldInfo::Enum(fields) => {
            let id_type = fields[0].ty;

            for field in fields.iter().skip(1) {
                if field.ty != id_type {
                    let msg = "All fields marked with `#[attribute_id]` must have the same type across all enum variants.";
                    return Err(Error::new_spanned(field.ty, msg));
                }
            }

            let mut aggregate_id_arms = Vec::new();
            let mut name_arms = Vec::new();

            for info in fields {
                let variant_ident = &info.variant.ident;
                let field_ident = info.name;

                let variant_attribute = utils::get_attribute(
                    &info.variant.attrs,
                    "domain_event",
                    info.variant.ident.span(),
                )?;

                let mut event_name: Option<LitStr> = None;
                variant_attribute.parse_nested_meta(|meta| {
                    if meta.path.is_ident("name") {
                        event_name = Some(meta.value()?.parse()?);
                    } else {
                        return Err(meta.error("unrecognized key for `#[domain_event]` attribute"));
                    }

                    Ok(())
                })?;

                let event_name = event_name.ok_or_else(|| {
                    Error::new(variant_attribute.path().span(), "`name` key is required")
                })?;

                aggregate_id_arms.push(quote! {
                    Self::#variant_ident { #field_ident, .. } => #field_ident
                });

                name_arms.push(quote! {
                    Self::#variant_ident { .. } => #event_name
                });
            }

            let expanded = quote! {

                impl ::nexus::core::Message for #name {
                }

                impl ::nexus::core::DomainEvent for #name {
                    type Id = #id_type;


                    fn aggregate_id(&self) -> &Self::Id {
                        match self {
                            #(#aggregate_id_arms),*
                        }
                    }

                    fn name(&self) -> &'static str {
                        match self {
                            #(#name_arms),*
                        }
                    }
                }
            };

            Ok(expanded)
        }
    }
}

fn parse_aggregate(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let attribute = utils::get_attribute(&ast.attrs, "aggregate", name.span())?;

    let mut id: Option<Type> = None;
    let mut event: Option<Type> = None;
    let mut state: Option<Type> = None;

    attribute.parse_nested_meta(|meta| {
        if meta.path.is_ident("id") {
            id = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("event") {
            event = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("state") {
            state = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("unrecognized key for `#[aggregate]` attribute"));
        }
        Ok(())
    })?;

    let id = id.ok_or_else(|| Error::new(attribute.path().span(), "`id` key is required"))?;

    let event =
        event.ok_or_else(|| Error::new(attribute.path().span(), "`event` key is required"))?;

    let state =
        state.ok_or_else(|| Error::new(attribute.path().span(), "`state` key is required"))?;

    let expanded = quote! {
            impl ::nexus::command::aggregate::AggregateType for #name {
                type Id = #id;
                type Event = #event;
                type State = #state;
            }
    };

    Ok(expanded)
}
