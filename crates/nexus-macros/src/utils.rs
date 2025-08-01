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
pub fn get_attribute<'a>(
    attributes: &'a [Attribute],
    name: &'a str,
    error_span: Span,
) -> Result<&'a Attribute> {
    attributes
        .iter()
        .find(|a| a.path().is_ident(name))
        .ok_or_else(|| {
            let msg = format!("missing required attribute `#[{name}]`");
            Error::new(error_span, msg)
        })
}

#[cfg(test)]
mod test {
    use super::*;
    use proc_macro2::Span;
    use syn::{Attribute, DeriveInput, parse_str};

    fn parse_attr(code: &str) -> Vec<Attribute> {
        let ast: DeriveInput = parse_str(code).expect("Failed to parse the test code");
        ast.attrs
    }

    #[test]
    fn should_find_attribute_when_present() {
        let attrs = parse_attr(
            r#"
        #[other_attr]
        #[command(result = User)]
        struct TestStruct;
        "#,
        );
        let span = Span::call_site();
        let result = get_attribute(&attrs, "command", span);
        assert!(result.is_ok());
        let found_attr = result.unwrap();
        assert!(found_attr.path().is_ident("command"));
    }
    #[test]
    fn should_return_error_when_absent() {
        let attrs = parse_attr(
            r#"
        #[other_attr]
        #[query(result = User)]
        struct TestStruct;
        "#,
        );
        let span = Span::call_site();
        let result = get_attribute(&attrs, "command", span);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "missing required attribute `#[command]`");
    }
}
