use proc_macro2::Span;
use syn::{Attribute, Data, Error, Fields, Result, Type, Variant};

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
            let msg = format!("missing required attribute `#[{}]`", name);
            Error::new(error_span, msg)
        })
}

pub struct FieldInfo<'a> {
    pub ty: &'a Type,
    pub variant: &'a Variant,
}

pub enum DataTypesFieldInfo<'a> {
    Struct { ty: &'a Type },
    Enum(Vec<FieldInfo<'a>>),
}

pub fn get_fields_info<'a>(
    data: &'a Data,
    attribute_name: &'a str,
    error_span: Span,
) -> Result<DataTypesFieldInfo<'a>> {
    match data {
        Data::Struct(s) => {
            let ty = find_in_fields(&s.fields, attribute_name, error_span)?;
            Ok(DataTypesFieldInfo::Struct { ty })
        }
        Data::Enum(e) => {
            let mut field_infos: Vec<FieldInfo<'a>> = Vec::new();
            for variant in &e.variants {
                let ty = find_in_fields(&variant.fields, attribute_name, error_span)?;
                field_infos.push(FieldInfo { ty, variant });
            }
            Ok(DataTypesFieldInfo::Enum(field_infos))
        }
        Data::Union(_) => Err(Error::new(error_span, "Unions are not supported.")),
    }
}

pub fn find_in_fields<'a>(
    fields: &'a Fields,
    attribute_name: &'a str,
    error_span: Span,
) -> Result<&'a Type> {
    let mut found_fields = Vec::new();

    for field in fields {
        if field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident(attribute_name))
        {
            field.ident.as_ref().ok_or_else(|| {
                let msg = format!(
                    "The `#[{attribute_name}]` attribute can only be placed on fields with names."
                );
                Error::new_spanned(field, msg)
            })?;
            found_fields.push(&field.ty);
        }
    }

    match found_fields.len() {
        1 => Ok(found_fields.pop().unwrap()), // Safe due to length check
        0 => {
            let msg = format!("A field must be marked with `#[{attribute_name}]`");
            Err(Error::new(error_span, msg))
        }
        _ => {
            let msg = format!("Only one field can be marked with `#[{attribute_name}]`");
            Err(Error::new(error_span, msg))
        }
    }
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
