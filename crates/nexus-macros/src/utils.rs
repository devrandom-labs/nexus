use proc_macro2::Span;
use syn::{Attribute, Error, Result};

/// Finds a specific attribute in a slice, returning a targeted error if not found.
///
/// # Arguments
/// * `attributes`: The slice of attributes to search (e.g., from a `DeriveInput`).
/// * `name`: The identifier of the attribute to find (e.g., "command").
/// * `error_span`: The span to attach the error to if the attribute is missing.
///   This should be the identifier of the item being processed (e.g., the struct's name).
///
/// # Returns
/// A `Result` containing a reference to the found attribute, or a `syn::Error`
/// pinpointing the error's location.
fn get_attribute<'a>(
    attributes: &'a [Attribute],
    name: &'a str,
    error_span: Span,
) -> Result<&'a Attribute> {
    attributes
        .iter()
        .find(|a| a.path().is_ident(name))
        .ok_or_else(|| {
            let msg = format!("missing required attribute `#[{}]`", name);
            Error::new(error_span, msg)
        })
}

#[cfg(test)]
mod tests {}
