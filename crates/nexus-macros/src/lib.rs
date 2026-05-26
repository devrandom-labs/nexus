use std::collections::{HashMap, HashSet};

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
/// - `new(id)` constructor
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

/// Generates a unit struct with inherent `upcast` and `current_version`
/// functions from annotated transform functions.
///
/// # Attributes
///
/// - `aggregate = Type` — the aggregate type these transforms belong to
/// - `error = Type` — the error type returned by transform functions
///
/// Each method must be annotated with `#[transform(...)]`:
/// - `event = "EventName"` — the event type this transform handles
/// - `from = N` — source schema version (>= 1)
/// - `to = N` — target schema version (must be `from + 1`)
/// - `rename = "NewName"` — optional event type rename
///
/// # Compile-time validation
///
/// - `from >= 1`
/// - `to == from + 1` for each transform (contiguity per step)
/// - No duplicate `(event, from)` pairs
/// - **Chain coverage**: for each event type, every schema version in
///   `[1, current_version]` is reachable via a contiguous chain — gaps
///   produce a compile error naming the missing step
///
/// # Emitted output
///
/// The macro emits a `pub struct <Name>;` plus an inherent impl block
/// carrying the user's transform functions (with `#[transform]` attrs
/// stripped) and two associated functions:
///
/// - `pub fn upcast<'a>(EventMorsel<'a>) -> Result<EventMorsel<'a>, Error>` —
///   runs the chain to current schema version. Associated (no `&self`)
///   so call sites are `OrderTransforms::upcast(morsel)` — a `'static`
///   function pointer pluggable into [`EventStore::load_with`].
/// - `pub fn current_version(event_type: &str) -> Option<Version>` — the
///   write-path schema-version stamp lookup. Also associated; call as
///   `OrderTransforms::current_version("EventName")`.
///
/// # Example
///
/// ```ignore
/// #[nexus::transforms(aggregate = Order, error = MyError)]
/// impl OrderTransforms {
///     #[transform(event = "OrderCreated", from = 1, to = 2)]
///     fn v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, MyError> {
///         Ok(payload.to_vec())
///     }
/// }
///
/// // Direct call:
/// let upgraded = OrderTransforms::upcast(morsel)?;
///
/// // Plugged into the facade:
/// let root = store.load_with(id, OrderTransforms::upcast).await?;
/// ```
#[proc_macro_attribute]
pub fn transforms(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = attr;
    let ast = parse_macro_input!(item as syn::ItemImpl);
    match parse_transforms(&ast, args.into()) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}

struct TransformDef {
    fn_name: syn::Ident,
    event_type: String,
    from_version: u64,
    to_version: u64,
    rename: Option<String>,
}

fn parse_transform_attr(method: &syn::ImplItemFn) -> Result<Option<TransformDef>> {
    let mut transform_attr = None;

    for attr in &method.attrs {
        if attr.path().is_ident("transform") {
            if transform_attr.is_some() {
                return Err(Error::new_spanned(attr, "duplicate #[transform] attribute"));
            }

            let mut event_type: Option<String> = None;
            let mut from_version: Option<u64> = None;
            let mut to_version: Option<u64> = None;
            let mut rename: Option<String> = None;

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("event") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    event_type = Some(lit.value());
                } else if meta.path.is_ident("from") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    from_version = Some(lit.base10_parse()?);
                } else if meta.path.is_ident("to") {
                    let value = meta.value()?;
                    let lit: syn::LitInt = value.parse()?;
                    to_version = Some(lit.base10_parse()?);
                } else if meta.path.is_ident("rename") {
                    let value = meta.value()?;
                    let lit: syn::LitStr = value.parse()?;
                    rename = Some(lit.value());
                } else {
                    return Err(meta.error("expected `event`, `from`, `to`, or `rename`"));
                }
                Ok(())
            })?;

            let event_type = event_type.ok_or_else(|| {
                Error::new_spanned(attr, "`event` is required in #[transform(...)]")
            })?;
            let from_version = from_version.ok_or_else(|| {
                Error::new_spanned(attr, "`from` is required in #[transform(...)]")
            })?;
            let to_version = to_version
                .ok_or_else(|| Error::new_spanned(attr, "`to` is required in #[transform(...)]"))?;

            transform_attr = Some(TransformDef {
                fn_name: method.sig.ident.clone(),
                event_type,
                from_version,
                to_version,
                rename,
            });
        }
    }

    Ok(transform_attr)
}

fn parse_transforms(
    ast: &syn::ItemImpl,
    args: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream> {
    // 1. Parse aggregate = Type, error = Type from outer attributes
    let mut aggregate_type: Option<Type> = None;
    let mut error_type: Option<Type> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("aggregate") {
            aggregate_type = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("error") {
            error_type = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("expected `aggregate` or `error`"));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, args)?;
    let _aggregate_type = aggregate_type
        .ok_or_else(|| Error::new(proc_macro2::Span::call_site(), "`aggregate` is required"))?;
    let error_type = error_type
        .ok_or_else(|| Error::new(proc_macro2::Span::call_site(), "`error` is required"))?;

    // 2. Get the struct name from the impl block
    let struct_ident = match &*ast.self_ty {
        syn::Type::Path(p) => {
            &p.path
                .segments
                .last()
                .ok_or_else(|| Error::new_spanned(&ast.self_ty, "expected a type name"))?
                .ident
        }
        _ => return Err(Error::new_spanned(&ast.self_ty, "expected a type name")),
    };

    // 3. Parse each method's #[transform] attributes
    let mut transforms = Vec::new();
    for item in &ast.items {
        let method = match item {
            syn::ImplItem::Fn(m) => m,
            _ => continue,
        };
        if let Some(def) = parse_transform_attr(method)? {
            transforms.push(def);
        }
    }

    // 4. Validate from >= 1
    for t in &transforms {
        if t.from_version < 1 {
            return Err(Error::new_spanned(&t.fn_name, "from version must be >= 1"));
        }
    }

    // 5. Validate to == from + 1
    for t in &transforms {
        if t.to_version != t.from_version + 1 {
            return Err(Error::new_spanned(
                &t.fn_name,
                format!(
                    "non-contiguous version: to ({}) must equal from + 1 ({})",
                    t.to_version,
                    t.from_version + 1,
                ),
            ));
        }
    }

    // 6. Validate no duplicate (event, from)
    let mut seen = HashSet::new();
    for t in &transforms {
        let key = (t.event_type.clone(), t.from_version);
        if !seen.insert(key) {
            return Err(Error::new_spanned(
                &t.fn_name,
                format!(
                    "duplicate transform for event '{}' at source version {}",
                    t.event_type, t.from_version,
                ),
            ));
        }
    }

    // 6b. Validate chain coverage per event type.
    //
    // For each event type, every schema version in [1, current_version]
    // must be reachable through a contiguous chain. Find the smallest gap
    // (i.e. the smallest `from` in [1, max_from] for which no transform
    // exists) and reject with a diagnostic naming the missing step.
    let mut from_versions_by_event: HashMap<String, HashSet<u64>> = HashMap::new();
    let mut max_from_by_event: HashMap<String, u64> = HashMap::new();
    for t in &transforms {
        from_versions_by_event
            .entry(t.event_type.clone())
            .or_default()
            .insert(t.from_version);
        let entry = max_from_by_event.entry(t.event_type.clone()).or_insert(0);
        if t.from_version > *entry {
            *entry = t.from_version;
        }
    }
    for t in &transforms {
        let Some(from_set) = from_versions_by_event.get(&t.event_type) else {
            continue;
        };
        let Some(&max_from) = max_from_by_event.get(&t.event_type) else {
            continue;
        };
        for v in 1..max_from {
            if !from_set.contains(&v) {
                return Err(Error::new_spanned(
                    &t.fn_name,
                    format!(
                        "transform chain gap for event '{}': missing step from version {} to version {} (chain must cover every version in [1, {}])",
                        t.event_type,
                        v,
                        v + 1,
                        max_from + 1,
                    ),
                ));
            }
        }
    }

    // 7. Build the original impl block with #[transform] attrs stripped
    let stripped_methods: Vec<_> = ast
        .items
        .iter()
        .map(|item| match item {
            syn::ImplItem::Fn(m) => {
                let mut method = m.clone();
                method.attrs.retain(|a| !a.path().is_ident("transform"));
                syn::ImplItem::Fn(method)
            }
            other => other.clone(),
        })
        .collect();

    // 8. Generate match arms for upcast()
    let match_arms: Vec<_> = transforms
        .iter()
        .map(|t| {
            let fn_name = &t.fn_name;
            let event_type = &t.event_type;
            let from_version = t.from_version;
            let to_version = t.to_version;
            let output_event_type = t.rename.as_deref().unwrap_or(&t.event_type);

            quote! {
                (#event_type, v) if v == ::nexus::Version::new(#from_version).expect("nonzero") => {
                    let payload = Self::#fn_name(morsel.payload())?;
                    ::nexus_store::upcasting::EventMorsel::new(
                        #output_event_type,
                        ::nexus::Version::new(#to_version).expect("nonzero"),
                        payload,
                    )
                }
            }
        })
        .collect();

    // 9. Compute max version per event type for current_version()
    let mut max_versions: HashMap<String, u64> = HashMap::new();
    for t in &transforms {
        let entry = max_versions.entry(t.event_type.clone()).or_insert(1);
        if t.to_version > *entry {
            *entry = t.to_version;
        }
        // Track renamed destination event types too
        if let Some(ref rename) = t.rename {
            let entry = max_versions.entry(rename.clone()).or_insert(1);
            if t.to_version > *entry {
                *entry = t.to_version;
            }
        }
    }

    let version_arms: Vec<_> = max_versions
        .iter()
        .map(|(event_type, version)| {
            quote! {
                #event_type => ::core::option::Option::Some(
                    ::nexus::Version::new(#version).expect("nonzero")
                )
            }
        })
        .collect();

    // 10. Emit: pub struct + impl block carrying user methods + the two
    //     generated associated functions. No trait impl — call sites use
    //     path syntax (`X::upcast(...)`, `X::current_version(...)`) which
    //     yields `'static` function pointers pluggable into the facade's
    //     `load_with` / `save_with` methods.
    let expanded = quote! {
        pub struct #struct_ident;

        impl #struct_ident {
            #(#stripped_methods)*

            /// Run all matching transforms until the morsel reaches the
            /// current schema version. Generated by `#[nexus::transforms]`.
            pub fn upcast<'a>(
                mut morsel: ::nexus_store::upcasting::EventMorsel<'a>,
            ) -> ::core::result::Result<
                ::nexus_store::upcasting::EventMorsel<'a>,
                #error_type,
            > {
                loop {
                    morsel = match (morsel.event_type(), morsel.schema_version()) {
                        #(#match_arms,)*
                        _ => break,
                    };
                }
                ::core::result::Result::Ok(morsel)
            }

            /// Current schema version for `event_type` (stamped on new
            /// events). `None` when the event type has no transforms.
            /// Generated by `#[nexus::transforms]`.
            #[must_use]
            pub fn current_version(event_type: &str) -> ::core::option::Option<::nexus::Version> {
                match event_type {
                    #(#version_arms,)*
                    _ => ::core::option::Option::None,
                }
            }
        }
    };

    Ok(expanded)
}

fn parse_aggregate(
    ast: &DeriveInput,
    args: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream> {
    let name = &ast.ident;
    let vis = &ast.vis;
    // Preserve user attributes (#[cfg(...)], #[doc = "..."], etc.)
    let user_attrs = &ast.attrs;

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

    let state_type = state_type.ok_or_else(|| Error::new(name.span(), "`state` is required"))?;
    let error_type = error_type.ok_or_else(|| Error::new(name.span(), "`error` is required"))?;
    let id_type = id_type.ok_or_else(|| Error::new(name.span(), "`id` is required"))?;

    let expanded = quote! {
        #(#user_attrs)*
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
