use apache_avro::{
    AvroResult, Schema,
    error::Details,
    util,
    validator::{
        EnumSymbolNameValidator, RecordFieldNameValidator, SchemaNameValidator,
        SchemaNamespaceValidator, set_enum_symbol_name_validator, set_record_field_name_validator,
        set_schema_name_validator, set_schema_namespace_validator,
    },
};
use std::sync::OnceLock;

struct Validator;

const NAME_PATTERN: &str = r"^((?P<namespace>([A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*)?)\.)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)$";
const NAMESPACE_PATTERN: &str = r"^([A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*)?$";

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn namespace(value: &str) -> bool {
    value.is_empty() || value.split('.').all(identifier)
}

impl SchemaNameValidator for Validator {
    fn validate(&self, value: &str) -> AvroResult<usize> {
        let index = value.rfind('.').map_or(0, |index| index + 1);
        if identifier(&value[index..]) && (index == 0 || namespace(&value[..index - 1])) {
            Ok(index)
        } else {
            Err(Details::InvalidSchemaName(value.into(), NAME_PATTERN).into())
        }
    }
}

impl SchemaNamespaceValidator for Validator {
    fn validate(&self, value: &str) -> AvroResult<()> {
        if namespace(value) {
            Ok(())
        } else {
            Err(Details::InvalidNamespace(value.into(), NAMESPACE_PATTERN).into())
        }
    }
}

impl EnumSymbolNameValidator for Validator {
    fn validate(&self, value: &str) -> AvroResult<()> {
        if identifier(value) {
            Ok(())
        } else {
            Err(Details::EnumSymbolName(value.into()).into())
        }
    }
}

impl RecordFieldNameValidator for Validator {
    fn validate(&self, value: &str) -> AvroResult<()> {
        if identifier(value) {
            Ok(())
        } else {
            Err(Details::FieldName(value.into()).into())
        }
    }
}

pub(crate) fn initialize() -> Result<(), String> {
    static INITIALIZED: OnceLock<Result<(), &'static str>> = OnceLock::new();
    INITIALIZED
        .get_or_init(|| {
            // Apache's default regex caches contain mutexes that can remain locked after fork.
            set_schema_name_validator(Box::new(Validator))
                .map_err(|_| "Apache schema-name validator was initialized before Avrocadabra")?;
            set_schema_namespace_validator(Box::new(Validator))
                .map_err(|_| "Apache namespace validator was initialized before Avrocadabra")?;
            set_enum_symbol_name_validator(Box::new(Validator))
                .map_err(|_| "Apache enum validator was initialized before Avrocadabra")?;
            set_record_field_name_validator(Box::new(Validator))
                .map_err(|_| "Apache field-name validator was initialized before Avrocadabra")?;
            util::max_allocation_bytes(util::DEFAULT_MAX_ALLOCATION_BYTES);
            util::set_serde_human_readable(util::DEFAULT_SERDE_HUMAN_READABLE);
            let _ = Schema::Null == Schema::Int;
            Ok(())
        })
        .map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Original;
    impl SchemaNameValidator for Original {}
    impl SchemaNamespaceValidator for Original {}
    impl EnumSymbolNameValidator for Original {}
    impl RecordFieldNameValidator for Original {}

    #[test]
    fn ascii_validation_matches_apache_regex_rules() {
        initialize().unwrap();
        let alphabet = ['A', 'z', '0', '_', '.', '-', 'é', '\n', ' '];
        let mut values = vec![String::new()];
        let mut frontier = vec![String::new()];
        for _ in 0..4 {
            frontier = frontier
                .iter()
                .flat_map(|prefix| {
                    alphabet
                        .iter()
                        .map(move |character| format!("{prefix}{character}"))
                })
                .collect();
            values.extend(frontier.iter().cloned());
        }
        values.extend(
            [
                "example.Name",
                "example.deep.Name",
                ".Name",
                "example.",
                ".example.Name",
                "__Name9",
            ]
            .map(str::to_owned),
        );
        for value in values {
            assert_eq!(
                SchemaNameValidator::validate(&Validator, &value).ok(),
                SchemaNameValidator::validate(&Original, &value).ok(),
                "schema name {value:?}"
            );
            assert_eq!(
                SchemaNamespaceValidator::validate(&Validator, &value).is_ok(),
                SchemaNamespaceValidator::validate(&Original, &value).is_ok(),
                "namespace {value:?}"
            );
            assert_eq!(
                EnumSymbolNameValidator::validate(&Validator, &value).is_ok(),
                EnumSymbolNameValidator::validate(&Original, &value).is_ok(),
                "enum symbol {value:?}"
            );
            assert_eq!(
                RecordFieldNameValidator::validate(&Validator, &value).is_ok(),
                RecordFieldNameValidator::validate(&Original, &value).is_ok(),
                "field name {value:?}"
            );
        }
    }

    #[test]
    fn initialization_is_idempotent_and_names_keep_enclosing_namespace_rules() {
        initialize().unwrap();
        initialize().unwrap();
        use apache_avro::schema::Name;
        for (name, enclosing, expected) in [
            ("Value", None, "Value"),
            ("Value", Some(""), "Value"),
            ("Value", Some("outer.ns"), "outer.ns.Value"),
            ("own.Value", Some("outer.ns"), "own.Value"),
            (".Value", Some("outer.ns"), "Value"),
        ] {
            assert_eq!(
                Name::new_with_enclosing_namespace(name, enclosing)
                    .unwrap()
                    .fullname(None),
                expected
            );
        }
    }
}
