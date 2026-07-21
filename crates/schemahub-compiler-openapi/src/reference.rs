//! Parsing and canonical rendering for the subset of OpenAPI `$ref` values
//! that SchemaHub can resolve through its logical schema dependency graph.

use crate::ast::{ExternalImport, SchemaRef};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ComponentReference {
    Local(String),
    External(ExternalImport),
}

/// Parse a source-level component reference.
///
/// SchemaHub supports local component references and external component
/// references whose URI path is a same-repository repository-root path, an
/// explicit `./` or `../` source-relative path, or the
/// `project/repository/schema` logical form understood by Core. Network URLs,
/// query strings, and arbitrary JSON Pointer fragments are rejected instead of
/// being silently misrepresented as registry dependencies.
pub(crate) fn parse_source_component_reference(
    reference: &str,
    component: &str,
) -> Result<ComponentReference, String> {
    let local_prefix = format!("#/components/{component}/");
    if let Some(encoded_name) = reference.strip_prefix(&local_prefix) {
        return decode_component_name(encoded_name).map(ComponentReference::Local);
    }

    if reference.starts_with('#') {
        return Err(format!(
            "unsupported OpenAPI $ref {reference:?}: expected {local_prefix}<name>"
        ));
    }

    let (path, fragment) = reference.split_once('#').ok_or_else(|| {
        format!(
            "unsupported external OpenAPI $ref {reference:?}: expected \
             <schema-path>#/components/{component}/<name>"
        )
    })?;
    validate_schemahub_path(path, reference)?;

    let external_prefix = format!("/components/{component}/");
    let encoded_name = fragment.strip_prefix(&external_prefix).ok_or_else(|| {
        format!(
            "unsupported external OpenAPI $ref {reference:?}: expected fragment \
             {external_prefix}<name>"
        )
    })?;
    let decl_name = decode_component_name(encoded_name)?;

    Ok(ComponentReference::External(ExternalImport {
        path: path.to_owned(),
        resolved_commit: String::new(),
        decl_name,
    }))
}

/// Interpret the compact value stored by the parameter/response/request-body
/// AST variants. Existing local blobs store a decoded component name; external
/// values retain their complete source `$ref`.
pub(crate) fn parse_stored_component_reference(
    stored: &str,
    component: &str,
) -> Result<ComponentReference, String> {
    if stored.contains('#') {
        parse_source_component_reference(stored, component)
    } else if stored.is_empty() || stored.chars().any(char::is_control) {
        Err("stored OpenAPI component reference is empty or contains control characters".into())
    } else {
        Ok(ComponentReference::Local(stored.to_owned()))
    }
}

pub(crate) fn parse_stored_schema_reference(
    reference: &SchemaRef,
) -> Result<ComponentReference, String> {
    let Some(external) = &reference.external_import else {
        return parse_stored_component_reference(&reference.local_name, "schemas");
    };

    let rendered =
        render_component_reference(&ComponentReference::External(external.clone()), "schemas");
    let ComponentReference::External(mut validated) =
        parse_source_component_reference(&rendered, "schemas")?
    else {
        return Err("stored external OpenAPI schema reference decoded as local".into());
    };
    validated
        .resolved_commit
        .clone_from(&external.resolved_commit);
    Ok(ComponentReference::External(validated))
}

pub(crate) fn render_stored_component_reference(stored: &str, component: &str) -> String {
    match parse_stored_component_reference(stored, component) {
        Ok(reference) => render_component_reference(&reference, component),
        // Decoding and read APIs reject malformed blobs. Keep the printer
        // total so conflict rendering can still expose damaged legacy bytes.
        Err(_) => stored.to_owned(),
    }
}

pub(crate) fn render_component_reference(
    reference: &ComponentReference,
    component: &str,
) -> String {
    match reference {
        ComponentReference::Local(name) => {
            format!("#/components/{component}/{}", encode_pointer_segment(name))
        }
        ComponentReference::External(import) => format!(
            "{}#/components/{component}/{}",
            import.path,
            encode_pointer_segment(&import.decl_name)
        ),
    }
}

fn validate_schemahub_path(path: &str, reference: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(':')
        || path.contains('?')
        || path.contains('\\')
        || path
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || path.split('/').any(str::is_empty)
    {
        return Err(format!(
            "external OpenAPI $ref {reference:?} must use a relative schema path or \
             project/repository/schema logical path (URLs, absolute paths, queries, \
             whitespace, and backslashes are unsupported)"
        ));
    }
    Ok(())
}

fn decode_component_name(encoded: &str) -> Result<String, String> {
    if encoded.is_empty() || encoded.contains('/') || encoded.contains('#') {
        return Err("OpenAPI component reference must name exactly one component".into());
    }

    let mut decoded = String::with_capacity(encoded.len());
    let mut chars = encoded.chars();
    while let Some(character) = chars.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            Some(other) => {
                return Err(format!(
                    "OpenAPI component reference contains invalid JSON Pointer escape ~{other}"
                ))
            }
            None => {
                return Err(
                    "OpenAPI component reference ends with an incomplete JSON Pointer escape"
                        .into(),
                )
            }
        }
    }
    Ok(decoded)
}

fn encode_pointer_segment(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_external_component_reference_into_live_schemahub_import() {
        // Arrange
        let source = "shared/common.yaml#/components/schemas/User~1Profile";

        // Act
        let parsed = parse_source_component_reference(source, "schemas").unwrap();

        // Assert
        assert_eq!(
            parsed,
            ComponentReference::External(ExternalImport {
                path: "shared/common.yaml".into(),
                resolved_commit: String::new(),
                decl_name: "User/Profile".into(),
            })
        );
    }

    #[test]
    fn component_reference_round_trip_escapes_json_pointer_name() {
        // Arrange
        let parsed = ComponentReference::Local("User/Profile~v1".into());

        // Act
        let rendered = render_component_reference(&parsed, "schemas");

        // Assert
        assert_eq!(rendered, "#/components/schemas/User~1Profile~0v1");
    }

    #[test]
    fn rejects_network_reference_instead_of_treating_it_as_logical_path() {
        // Arrange
        let source = "https://example.com/common.yaml#/components/schemas/User";

        // Act
        let result = parse_source_component_reference(source, "schemas");

        // Assert
        assert!(result.is_err());
    }
}
