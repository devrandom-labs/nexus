use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Error, Result, Type, parse_macro_input};

/// Attribute macro that transforms a unit struct into an aggregate newtype.
///
/// Usage:
/// ```ignore
/// #[nexus::aggregate(state = MyState, error = MyError, id = MyId)]
/// struct MyAggregate;
/// ```
///
/// Generates:
/// - Replaces the unit struct with a newtype wrapping `AggregateRoot<Self>`
/// - `impl Aggregate` with the specified associated types
/// - `impl AggregateEntity` with `root()`/`root_mut()` delegation
/// - `new(id)` and `load_from_events(id, events)` constructors
/// - `impl Debug`
#[proc_macro_attribute]
pub fn aggregate(attr: TokenStream, item: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(item as DeriveInput);
    let args = attr;
    match parse_aggregate(&ast, args.into()) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}

#[proc_macro_derive(DomainEvent)]
pub fn domain_event(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match parse_domain_event(&ast) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}

fn parse_domain_event(ast: &DeriveInput) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    match &ast.data {
        Data::Enum(data_enum) => {
            if data_enum.variants.is_empty() {
                return Err(Error::new(
                    name.span(),
                    "DomainEvent enum must have at least one variant.",
                ));
            }

            let variant_arms: Vec<_> = data_enum
                .variants
                .iter()
                .map(|variant| {
                    let variant_ident = &variant.ident;
                    let variant_name = variant_ident.to_string();
                    match &variant.fields {
                        syn::Fields::Unit => {
                            quote! { #name::#variant_ident => #variant_name }
                        }
                        syn::Fields::Unnamed(_) => {
                            quote! { #name::#variant_ident(..) => #variant_name }
                        }
                        syn::Fields::Named(_) => {
                            quote! { #name::#variant_ident { .. } => #variant_name }
                        }
                    }
                })
                .collect();

            let expanded = quote! {
                impl ::nexus::Message for #name {}

                impl ::nexus::DomainEvent for #name {
                    fn name(&self) -> &'static str {
                        match self {
                            #(#variant_arms),*
                        }
                    }
                }
            };

            Ok(expanded)
        }
        Data::Struct(_) => Err(Error::new(
            name.span(),
            "DomainEvent derive requires an enum. Wrap event structs in an enum: `enum MyEvent { Created(Created), ... }`",
        )),
        Data::Union(_) => Err(Error::new(name.span(), "Unions are not supported.")),
    }
}

fn parse_aggregate(ast: &DeriveInput, args: proc_macro2::TokenStream) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let vis = &ast.vis;

    // Only unit structs allowed
    match &ast.data {
        Data::Struct(data) => {
            if !data.fields.is_empty() {
                return Err(Error::new(
                    name.span(),
                    "aggregate macro requires a unit struct (no fields).",
                ));
            }
        }
        _ => {
            return Err(Error::new(
                name.span(),
                "aggregate macro only works on unit structs.",
            ));
        }
    }

    // Parse state = ..., error = ..., id = ... from attribute args
    let mut state_type: Option<Type> = None;
    let mut error_type: Option<Type> = None;
    let mut id_type: Option<Type> = None;

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("state") {
            state_type = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("error") {
            error_type = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("id") {
            id_type = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("expected `state`, `error`, or `id`"));
        }
        Ok(())
    });

    syn::parse::Parser::parse2(parser, args)?;

    let state_type = state_type
        .ok_or_else(|| Error::new(name.span(), "`state` is required"))?;
    let error_type = error_type
        .ok_or_else(|| Error::new(name.span(), "`error` is required"))?;
    let id_type = id_type
        .ok_or_else(|| Error::new(name.span(), "`id` is required"))?;

    let expanded = quote! {
        #vis struct #name(::nexus::AggregateRoot<#name>);

        impl ::nexus::Aggregate for #name {
            type State = #state_type;
            type Error = #error_type;
            type Id = #id_type;
        }

        impl ::nexus::AggregateEntity for #name {
            fn root(&self) -> &::nexus::AggregateRoot<Self> {
                &self.0
            }
            fn root_mut(&mut self) -> &mut ::nexus::AggregateRoot<Self> {
                &mut self.0
            }
        }

        impl #name {
            /// Create a new aggregate with default state.
            #[must_use]
            #vis fn new(id: #id_type) -> Self {
                Self(::nexus::AggregateRoot::new(id))
            }

            /// Rehydrate from persisted events.
            ///
            /// # Errors
            ///
            /// Returns [`nexus::KernelError::VersionMismatch`] if event versions
            /// are not strictly sequential.
            #vis fn load_from_events(
                id: #id_type,
                events: impl IntoIterator<Item = ::nexus::VersionedEvent<::nexus::EventOf<#name>>>,
            ) -> ::std::result::Result<Self, ::nexus::KernelError> {
                ::nexus::AggregateRoot::load_from_events(id, events).map(Self)
            }
        }

        impl ::std::fmt::Debug for #name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                // Redacted: only shows aggregate name, id, and version.
                // Internal state is NOT exposed to prevent information leakage
                // in logs, error messages, and panic output.
                f.debug_struct(stringify!(#name))
                    .field("id", self.root().id())
                    .field("version", &self.root().version())
                    .finish_non_exhaustive()
            }
        }
    };

    Ok(expanded)
}
