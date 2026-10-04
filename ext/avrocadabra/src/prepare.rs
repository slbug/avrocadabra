use crate::guard::Limits;
use apache_avro::{
    Schema,
    schema::{Name, ResolvedSchema},
};
use serde_json::{Map, Value as Json, json};
use std::collections::{HashMap, HashSet};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_NODES: usize = 65_536;
const MAX_JSON_DEPTH: usize = 64;
type Fields = HashMap<String, HashMap<String, FieldMetadata>>;

struct FieldMetadata {
    default: Option<Json>,
    aliases: Vec<String>,
}

pub fn schemas(json: &str, references: &[String], limits: Limits) -> Result<Vec<Schema>, String> {
    crate::validation::initialize()?;
    if limits.max_depth > 128 {
        return Err("schema max_depth exceeds 128".into());
    }
    if references.len() >= MAX_NODES {
        return Err("schema exceeds node limit".into());
    }
    references
        .iter()
        .try_fold(json.len(), |total, value| total.checked_add(value.len()))
        .filter(|&total| total <= MAX_BYTES)
        .ok_or("schemas exceed 1 MiB")?;
    let mut nodes = 0;
    let mut inputs = Vec::with_capacity(references.len() + 1);
    for input in references.iter().map(String::as_str).chain([json]) {
        let tree: Json =
            serde_json::from_str(input).map_err(|error| format!("invalid schema JSON: {error}"))?;
        check_json(&tree, 0, &mut nodes)?;
        inputs.push(tree);
    }
    let mut normalizer = Normalizer {
        names: HashSet::new(),
        reserved: HashSet::new(),
        fields: HashMap::new(),
        errors: HashSet::new(),
        name_bytes: 0,
        limits,
    };
    let root_index = inputs.len() - 1;
    for (index, input) in inputs.iter_mut().enumerate() {
        if index != root_index && !is_named(input) {
            return Err("references must contain named record, enum, or fixed schemas".into());
        }
        normalizer.schema(input, None)?;
    }
    let mut definitions = HashMap::new();
    for input in &inputs {
        index_definitions(input, &mut definitions);
    }
    let mut expander = Expander {
        definitions,
        defined: HashSet::new(),
        depth: limits.max_depth.min(64),
    };
    let mut dependencies: Vec<_> = inputs[..root_index].iter().collect();
    dependencies.sort_by_key(|schema| schema.get("name").and_then(Json::as_str));
    let mut fields = Vec::with_capacity(inputs.len());
    let mut nodes = 0;
    for (index, input) in dependencies
        .into_iter()
        .chain([&inputs[root_index]])
        .enumerate()
    {
        let expanded = expander.schema(input, 0)?;
        check_json(&expanded, 0, &mut nodes)?;
        fields.push(json!({"name": format!("schema_{index}"), "type": expanded}));
    }
    let mut suffix = 0;
    let bundle_name = loop {
        let name = format!("__AvrocadabraSchemaBundle{suffix}");
        if !normalizer.reserved.contains(&name) {
            break name;
        }
        suffix += 1;
    };
    let bundle = json!({"type":"record", "name":bundle_name, "namespace":"", "fields":fields});
    let Schema::Record(bundle) = Schema::parse(&bundle).map_err(|error| error.to_string())? else {
        return Err("invalid internal schema bundle".into());
    };
    let mut schemas: Vec<_> = bundle
        .fields
        .into_iter()
        .map(|field| field.schema)
        .collect();
    for schema in &mut schemas {
        restore_metadata(schema, &mut normalizer.fields, &mut normalizer.errors)?;
    }
    if !normalizer.fields.is_empty() || !normalizer.errors.is_empty() {
        return Err("schema metadata could not be associated with its records".into());
    }
    let resolved = ResolvedSchema::new_with_schemata(schemas.iter().collect())
        .map_err(|error| error.to_string())?;
    crate::resolution::validate_defaults(&schemas, resolved.get_names(), limits)?;
    Ok(schemas)
}

fn check_json(value: &Json, depth: usize, nodes: &mut usize) -> Result<(), String> {
    *nodes += 1;
    if depth > MAX_JSON_DEPTH || *nodes > MAX_NODES {
        return Err("schema exceeds nesting or node limit".into());
    }
    match value {
        Json::Array(values) => {
            for value in values {
                check_json(value, depth + 1, nodes)?;
            }
        }
        Json::Object(values) => {
            for value in values.values() {
                check_json(value, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn primitive(name: &str) -> bool {
    matches!(
        name,
        "null" | "boolean" | "int" | "long" | "float" | "double" | "bytes" | "string"
    )
}

fn is_named(schema: &Json) -> bool {
    matches!(
        schema.get("type").and_then(Json::as_str),
        Some("record" | "error" | "enum" | "fixed")
    )
}

struct Normalizer {
    names: HashSet<String>,
    reserved: HashSet<String>,
    fields: Fields,
    errors: HashSet<String>,
    name_bytes: usize,
    limits: Limits,
}

impl Normalizer {
    fn count_name(&mut self, length: usize) -> Result<(), String> {
        self.name_bytes = self
            .name_bytes
            .checked_add(length)
            .filter(|&bytes| bytes <= MAX_BYTES)
            .ok_or("normalized schema names exceed 1 MiB")?;
        Ok(())
    }

    fn schema(&mut self, schema: &mut Json, namespace: Option<&str>) -> Result<(), String> {
        match schema {
            Json::String(name) if !primitive(name) => {
                let full = Name::new_with_enclosing_namespace(name.as_str(), namespace)
                    .map_err(|error| error.to_string())?
                    .fullname(None);
                self.count_name(full.len())?;
                *name = full;
            }
            Json::String(_) => {}
            Json::Array(branches) => {
                if branches.is_empty() {
                    return Err("union must contain at least one branch".into());
                }
                for branch in branches {
                    self.schema(branch, namespace)?;
                }
            }
            Json::Object(object) => {
                let is_error = object.get("type").and_then(Json::as_str) == Some("error");
                if is_error {
                    object.insert("type".into(), Json::String("record".into()));
                }
                self.logical(object)?;
                let kind = object.get("type").and_then(Json::as_str).map(str::to_owned);
                match kind.as_deref() {
                    Some("record" | "enum" | "fixed") => {
                        let name = object
                            .get("name")
                            .and_then(Json::as_str)
                            .ok_or("named schema requires a name")?;
                        let enclosing = match object.get("namespace") {
                            Some(Json::String(namespace)) => Some(namespace.as_str()),
                            Some(_) => return Err("schema namespace must be a string".into()),
                            None => namespace,
                        };
                        let name = Name::new_with_enclosing_namespace(name, enclosing)
                            .map_err(|error| error.to_string())?;
                        let fullname = name.fullname(None);
                        if is_error {
                            self.errors.insert(fullname.clone());
                        }
                        self.count_name(fullname.len())?;
                        if !self.names.insert(fullname.clone()) {
                            return Err(format!("duplicate named schema {fullname}"));
                        }
                        self.reserved.insert(fullname.clone());
                        if let Some(Json::Array(aliases)) = object.get("aliases") {
                            for alias in aliases {
                                if let Some(alias) = alias.as_str() {
                                    let alias =
                                        Name::new_with_enclosing_namespace(alias, name.namespace())
                                            .map_err(|error| error.to_string())?;
                                    let alias = alias.fullname(None);
                                    self.count_name(alias.len())?;
                                    self.reserved.insert(alias);
                                }
                            }
                        }
                        object.insert("name".into(), Json::String(fullname.clone()));
                        object.insert("namespace".into(), Json::String(String::new()));
                        if kind.as_deref() == Some("enum")
                            && object
                                .get("default")
                                .is_some_and(|value| !value.is_string())
                        {
                            return Err("enum default must be a symbol string".into());
                        }
                        if kind.as_deref() == Some("record") {
                            let fields = object
                                .get_mut("fields")
                                .and_then(Json::as_array_mut)
                                .ok_or("record fields must be an Array")?;
                            for field in fields {
                                let field = field
                                    .as_object_mut()
                                    .ok_or("record field must be an object")?;
                                let field_name = field
                                    .get("name")
                                    .and_then(Json::as_str)
                                    .ok_or("record field requires a name")?
                                    .to_owned();
                                let default = field.remove("default");
                                let aliases = field
                                    .remove("aliases")
                                    .and_then(|value| value.as_array().cloned())
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter_map(|value| value.as_str().map(str::to_owned))
                                    .collect::<Vec<_>>();
                                if default.is_some() || !aliases.is_empty() {
                                    self.fields
                                        .entry(fullname.clone())
                                        .or_default()
                                        .insert(field_name, FieldMetadata { default, aliases });
                                }
                                self.schema(
                                    field
                                        .get_mut("type")
                                        .ok_or("record field requires a type")?,
                                    name.namespace(),
                                )?;
                            }
                        }
                    }
                    Some("array") => self.schema(
                        object.get_mut("items").ok_or("array requires items")?,
                        namespace,
                    )?,
                    Some("map") => self.schema(
                        object.get_mut("values").ok_or("map requires values")?,
                        namespace,
                    )?,
                    Some(_) | None => self.schema(
                        object.get_mut("type").ok_or("schema requires a type")?,
                        namespace,
                    )?,
                }
            }
            _ => return Err("schema must be a type name, object, or union Array".into()),
        }
        Ok(())
    }

    fn logical(&self, object: &mut Map<String, Json>) -> Result<(), String> {
        let kind = object.get("type").and_then(Json::as_str);
        let fixed_size = if kind == Some("fixed") {
            let size = object
                .get("size")
                .and_then(Json::as_u64)
                .ok_or("fixed size must be a nonnegative integer")?;
            if size > self.limits.max_bytes as u64 {
                return Err("fixed size exceeds max_bytes".into());
            }
            Some(size)
        } else {
            None
        };
        if object.get("logicalType").and_then(Json::as_str) != Some("decimal") {
            return Ok(());
        }
        let precision = object.get("precision").and_then(Json::as_u64);
        let scale = object.get("scale").map_or(Some(0), Json::as_u64);
        let valid = matches!(kind, Some("bytes" | "fixed"))
            && matches!((precision, scale), (Some(p), Some(s)) if p > 0 && s <= p
                && fixed_size.is_none_or(|size| p <= fixed_decimal_capacity(size)));
        if !valid {
            object.remove("logicalType");
        } else if precision.is_some_and(|value| value > 4096) {
            return Err("decimal precision exceeds 4096 digit limit".into());
        }
        Ok(())
    }
}

fn fixed_decimal_capacity(size: u64) -> u64 {
    let bits = size.saturating_mul(8).saturating_sub(1);
    // floor(bits * log10(2)); 28 decimal places keep the integer boundary exact
    // throughout the 64 MiB fixed-size bound without exponentiating schema input.
    (u128::from(bits) * 3_010_299_956_639_811_952_137_388_947 / 10_u128.pow(28)) as u64
}

fn index_definitions<'a>(schema: &'a Json, definitions: &mut HashMap<&'a str, &'a Json>) {
    match schema {
        Json::Array(branches) => {
            for branch in branches {
                index_definitions(branch, definitions);
            }
        }
        Json::Object(object) => {
            let kind = object.get("type").and_then(Json::as_str);
            if matches!(kind, Some("record" | "enum" | "fixed"))
                && let Some(name) = object.get("name").and_then(Json::as_str)
            {
                definitions.insert(name, schema);
            }
            match kind {
                Some("record") => {
                    if let Some(Json::Array(fields)) = object.get("fields") {
                        for field in fields {
                            if let Some(schema) = field.get("type") {
                                index_definitions(schema, definitions);
                            }
                        }
                    }
                }
                Some("array") => {
                    if let Some(schema) = object.get("items") {
                        index_definitions(schema, definitions);
                    }
                }
                Some("map") => {
                    if let Some(schema) = object.get("values") {
                        index_definitions(schema, definitions);
                    }
                }
                _ => {
                    if let Some(schema) = object.get("type") {
                        index_definitions(schema, definitions);
                    }
                }
            }
        }
        _ => {}
    }
}

struct Expander<'a> {
    definitions: HashMap<&'a str, &'a Json>,
    defined: HashSet<&'a str>,
    depth: usize,
}

impl<'a> Expander<'a> {
    fn schema(&mut self, schema: &'a Json, depth: usize) -> Result<Json, String> {
        if depth > self.depth {
            return Err("schema dependency expansion exceeds maximum depth".into());
        }
        match schema {
            Json::String(name) => {
                if primitive(name) || self.defined.contains(name.as_str()) {
                    return Ok(schema.clone());
                }
                let definition = self
                    .definitions
                    .get(name.as_str())
                    .copied()
                    .ok_or_else(|| format!("unresolved named schema {name}"))?;
                self.schema(definition, depth)
            }
            Json::Array(branches) => branches
                .iter()
                .map(|schema| self.schema(schema, depth + 1))
                .collect::<Result<Vec<_>, _>>()
                .map(Json::Array),
            Json::Object(object) => {
                let kind = object.get("type").and_then(Json::as_str);
                if matches!(kind, Some("record" | "enum" | "fixed")) {
                    let name = object
                        .get("name")
                        .and_then(Json::as_str)
                        .ok_or("named schema requires name")?;
                    if !self.defined.insert(name) {
                        return Ok(Json::String(name.into()));
                    }
                }
                let child_key = match kind {
                    Some("record") => "fields",
                    Some("array") => "items",
                    Some("map") => "values",
                    Some("enum" | "fixed") => return Ok(schema.clone()),
                    _ => "type",
                };
                let mut output: Map<_, _> = object
                    .iter()
                    .filter(|(key, _)| key.as_str() != child_key)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                let child = object
                    .get(child_key)
                    .ok_or_else(|| format!("schema requires {child_key}"))?;
                let expanded = if child_key == "fields" {
                    let fields = child.as_array().ok_or("record requires fields")?;
                    let mut output = Vec::with_capacity(fields.len());
                    for field in fields {
                        let field = field.as_object().ok_or("record field requires object")?;
                        let mut output_field: Map<_, _> = field
                            .iter()
                            .filter(|(key, _)| key.as_str() != "type")
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect();
                        let schema = field.get("type").ok_or("record field requires type")?;
                        output_field.insert("type".into(), self.schema(schema, depth + 1)?);
                        output.push(Json::Object(output_field));
                    }
                    Json::Array(output)
                } else {
                    self.schema(child, depth + usize::from(child_key != "type"))?
                };
                output.insert(child_key.into(), expanded);
                Ok(Json::Object(output))
            }
            _ => Err("invalid schema".into()),
        }
    }
}

fn restore_metadata(
    schema: &mut Schema,
    fields: &mut Fields,
    errors: &mut HashSet<String>,
) -> Result<(), String> {
    match schema {
        Schema::Record(record) => {
            if errors.remove(record.name.as_ref()) {
                record
                    .attributes
                    .insert("type".into(), Json::String("error".into()));
            }
            let mut metadata = fields.remove(record.name.as_ref()).unwrap_or_default();
            for field in &mut record.fields {
                if let Some(metadata) = metadata.remove(&field.name) {
                    field.default = metadata.default;
                    field.aliases = metadata.aliases;
                }
                restore_metadata(&mut field.schema, fields, errors)?;
            }
            if !metadata.is_empty() {
                return Err("schema metadata references missing record fields".into());
            }
        }
        Schema::Array(array) => restore_metadata(&mut array.items, fields, errors)?,
        Schema::Map(map) => restore_metadata(&mut map.types, fields, errors)?,
        Schema::Union(union) => {
            // UnionSchema has no mutable variant access.
            let mut variants = union.variants().to_vec();
            for schema in &mut variants {
                restore_metadata(schema, fields, errors)?;
            }
            *union = apache_avro::schema::UnionSchema::new(variants)
                .map_err(|error| error.to_string())?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use apache_avro::{reader::datum::GenericDatumReader, types::Value};
    use num_bigint::BigInt;

    fn limits() -> Limits {
        Limits {
            max_depth: 64,
            max_bytes: 1_000_000,
            max_items: 10_000,
        }
    }

    fn read(root: &str, references: &[String], bytes: &[u8]) -> Value {
        let schemas = schemas(root, references, limits()).unwrap();
        let resolved = ResolvedSchema::new_with_schemata(schemas.iter().collect()).unwrap();
        let root = schemas.last().unwrap();
        crate::guard::prefix(root, resolved.get_names(), bytes, limits()).unwrap();
        GenericDatumReader::builder(root)
            .resolved_writer_schemata(resolved)
            .build()
            .unwrap()
            .read_value(&mut &bytes[..])
            .unwrap()
    }

    #[test]
    fn primitives_records_and_recursive_roots_keep_the_requested_root_last() {
        assert_eq!(read(r#""int""#, &[], &[2]), Value::Int(1));
        assert_eq!(
            read(
                r#"{"type":"record","name":"R","fields":[{"name":"next","type":["null","R"]}]}"#,
                &[],
                &[0]
            ),
            Value::Record(vec![(
                "next".into(),
                Value::Union(0, Box::new(Value::Null))
            )])
        );
    }

    #[test]
    fn forward_and_reverse_dependency_order_are_equivalent() {
        let a = r#"{"type":"record","name":"A","fields":[{"name":"b","type":"B"}]}"#.to_owned();
        let b = r#"{"type":"record","name":"B","fields":[{"name":"v","type":"int"}]}"#.to_owned();
        let forward = read(r#""A""#, &[a.clone(), b.clone()], &[14]);
        assert_eq!(forward, read(r#""A""#, &[b, a], &[14]));
        assert_eq!(
            forward,
            Value::Record(vec![(
                "b".into(),
                Value::Record(vec![("v".into(), Value::Int(7))])
            )])
        );
    }

    #[test]
    fn mutual_dependencies_are_defined_before_their_references_are_resolved() {
        let a = r#"{"type":"record","name":"A","fields":[{"name":"b","type":["null","B"]}]}"#
            .to_owned();
        let b = r#"{"type":"record","name":"B","fields":[{"name":"a","type":["null","A"]}]}"#
            .to_owned();
        assert_eq!(
            read(r#""A""#, &[a.clone(), b.clone()], &[0]),
            read(r#""A""#, &[b, a], &[0])
        );
    }

    #[test]
    fn root_can_participate_in_a_dependency_cycle() {
        let root = r#"{"type":"record","name":"Root","fields":[{"name":"child","type":"Child"}]}"#;
        let child =
            r#"{"type":"record","name":"Child","fields":[{"name":"root","type":["null","Root"]}]}"#
                .into();
        assert_eq!(
            read(root, &[child], &[0]),
            Value::Record(vec![(
                "child".into(),
                Value::Record(vec![(
                    "root".into(),
                    Value::Union(0, Box::new(Value::Null))
                )])
            )])
        );
    }

    #[test]
    fn namespaces_survive_dependency_embedding() {
        let root = r#"{"type":"record","name":"outer.Root","fields":[{"name":"other","type":"other.Value"}]}"#;
        let dependency = r#"{"type":"record","name":"other.Value","fields":[
            {"name":"f","type":{"type":"fixed","name":"F","size":1}}, {"name":"again","type":"F"}]}"#.into();
        assert!(matches!(
            read(root, &[dependency], &[1, 2]),
            Value::Record(_)
        ));
    }

    #[test]
    fn defaults_and_metadata_are_not_mistaken_for_logical_schemas() {
        let schema = r#"{"type":"record","name":"R","metadata":{"logicalType":"big-decimal"},"fields":[
            {"name":"v","type":{"type":"map","values":"string"},"default":{"logicalType":"big-decimal"}},
            {"name":"b","type":"bytes","default":"\u00ff"},
            {"name":"u","type":["null","int"],"default":7}]}"#;
        let parsed = schemas(schema, &[], limits()).unwrap();
        let Schema::Record(root) = parsed.last().unwrap() else {
            panic!("record expected");
        };
        assert_eq!(
            root.fields[0].default,
            Some(json!({"logicalType":"big-decimal"}))
        );
        assert_eq!(root.fields[1].default, Some(json!("ÿ")));
        assert_eq!(root.fields[2].default, Some(json!(7)));
    }

    #[test]
    fn defaults_inside_union_record_definitions_are_restored() {
        let schema = r#"["null",{"type":"record","name":"R","fields":[{"name":"v","type":"int","default":1}]}]"#;
        let parsed = schemas(schema, &[], limits()).unwrap();
        let Schema::Union(union) = parsed.last().unwrap() else {
            panic!("union expected");
        };
        let Schema::Record(record) = &union.variants()[1] else {
            panic!("record expected");
        };
        assert_eq!(record.fields[0].default, Some(json!(1)));
    }

    #[test]
    fn invalid_defaults_and_empty_unions_fail_during_preparation() {
        for schema in [
            "[]",
            r#"{"type":"record","name":"R","fields":[{"name":"v","type":"int","default":1.5}]}"#,
            r#"{"type":"record","name":"R","fields":[{"name":"v","type":"boolean","default":"false"}]}"#,
            r#"{"type":"record","name":"R","fields":[{"name":"v","type":["null","int"],"default":"no"}]}"#,
            r#"{"type":"enum","name":"E","symbols":["A"],"default":1}"#,
            r#"{"type":"record","name":"R","fields":[null]}"#,
        ] {
            assert!(schemas(schema, &[], limits()).is_err(), "{schema}");
        }
    }

    #[test]
    fn invalid_decimal_annotations_fall_back_to_the_physical_schema() {
        for schema in [
            r#"{"type":"bytes","logicalType":"decimal"}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":0}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":-1}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":2.5}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":"2"}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":2,"scale":-1}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":2,"scale":null}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":2,"scale":3}"#,
            r#"{"type":"bytes","logicalType":"decimal","precision":4097,"scale":4098}"#,
        ] {
            assert_eq!(schemas(schema, &[], limits()).unwrap(), vec![Schema::Bytes]);
        }
        for schema in [
            r#"{"type":"fixed","name":"D","size":1,"logicalType":"decimal","precision":3}"#,
            r#"{"type":"fixed","name":"D","size":1,"logicalType":"decimal","precision":18446744073709551615}"#,
        ] {
            assert!(matches!(
                schemas(schema, &[], limits()).unwrap()[0],
                Schema::Fixed(_)
            ));
        }
        assert_eq!(
            schemas(
                r#"{"type":"string","logicalType":"decimal","precision":2}"#,
                &[],
                limits()
            )
            .unwrap(),
            vec![Schema::String]
        );
    }

    #[test]
    fn valid_decimal_annotations_still_obey_resource_limits() {
        for schema in [
            r#"{"type":"bytes","logicalType":"decimal","precision":4097}"#,
            r#"{"type":"fixed","name":"D","size":2048,"logicalType":"decimal","precision":4097}"#,
        ] {
            assert!(schemas(schema, &[], limits()).unwrap_err().contains("4096"));
        }
        assert!(
            schemas(
                r#"{"type":"fixed","name":"D","size":1,"logicalType":"decimal","precision":2}"#,
                &[],
                limits()
            )
            .is_ok()
        );
    }

    #[test]
    fn fixed_decimal_capacity_matches_integer_boundaries() {
        assert_eq!(fixed_decimal_capacity(0), 0);
        for size in [1, 2, 4, 16, 128, 512, 1701, 1702, 4096] {
            let capacity = fixed_decimal_capacity(size) as u32;
            let bits = size * 8 - 1;
            assert!(BigInt::from(10_u8).pow(capacity).bits() <= bits);
            assert!(BigInt::from(10_u8).pow(capacity + 1).bits() > bits);
        }
    }

    #[test]
    fn big_decimal_uses_value_scale_and_ignores_schema_precision_metadata() {
        for schema in [
            r#"{"type":"bytes","logicalType":"big-decimal"}"#,
            r#"{"type":"bytes","logicalType":"big-decimal","precision":0,"scale":-5000}"#,
        ] {
            assert_eq!(
                schemas(schema, &[], limits()).unwrap(),
                vec![Schema::BigDecimal]
            );
        }
        assert_eq!(
            schemas(
                r#"{"type":"string","logicalType":"big-decimal"}"#,
                &[],
                limits()
            )
            .unwrap(),
            vec![Schema::String]
        );
    }

    #[test]
    fn big_decimal_defaults_validate_inner_framing_and_share_the_byte_budget() {
        let schema = |default: Json| {
            json!({"type":"record","name":"R","fields":[
                {"name":"v","type":{"type":"bytes","logicalType":"big-decimal"},"default":default}
            ]})
            .to_string()
        };
        for scale in [0, 5000, -5000, i64::MIN, i64::MAX] {
            let decimal = apache_avro::BigDecimal::new((-129).into(), scale);
            let bytes = crate::big_decimal::encode(&decimal).unwrap();
            let default: String = bytes.into_iter().map(char::from).collect();
            assert!(schemas(&schema(json!(default)), &[], limits()).is_ok());
        }
        for default in [
            json!(7),
            json!(""),
            json!("\u{0}\u{0}"),
            json!("\u{2}\u{7}"),
            json!("\u{2}\u{7}\u{4}\u{0}"),
            json!("Ā"),
        ] {
            let error = schemas(&schema(default), &[], limits()).unwrap_err();
            assert!(error.starts_with("$.v:"), "{error}");
        }
        let schema = json!({"type":"record","name":"R","fields":[
            {"name":"a","type":{"type":"bytes","logicalType":"big-decimal"},"default":"\u{2}\u{7}\u{4}"},
            {"name":"b","type":{"type":"bytes","logicalType":"big-decimal"},"default":"\u{2}\u{7}\u{4}"}
        ]}).to_string();
        let mut limits = limits();
        limits.max_bytes = 5;
        assert!(
            schemas(&schema, &[], limits)
                .unwrap_err()
                .contains("byte count")
        );
    }

    #[test]
    fn long_dependency_chains_fail_without_entering_apache_parser() {
        let references: Vec<_> = (0..512).map(|index| json!({"type":"record","name":format!("R{index:04}"),
            "fields":[{"name":"next","type":if index == 511 {"null".into()} else {format!("R{:04}", index + 1)}}]}).to_string()).collect();
        let error = schemas(r#""R0000""#, &references, limits()).unwrap_err();
        assert!(error.contains("depth") || error.contains("nesting"));
        let mut reverse = references;
        reverse.reverse();
        assert!(schemas(r#""R0000""#, &reverse, limits()).is_err());
    }

    #[test]
    fn implicit_default_expansion_is_bounded_before_any_apache_default_validation() {
        let mut references = vec![r#"{"type":"record","name":"Tree00","fields":[]}"#.into()];
        for level in 1..24 {
            references.push(
                json!({"type":"record","name":format!("Tree{level:02}"),"fields":[
                {"name":"left","type":format!("Tree{:02}",level-1),"default":{}},
                {"name":"right","type":format!("Tree{:02}",level-1),"default":{}}]})
                .to_string(),
            );
        }
        let error = schemas(r#""Tree23""#, &references, limits()).unwrap_err();
        assert!(error.contains("item count"));
    }

    #[test]
    fn unknown_duplicate_and_non_named_references_fail_cleanly() {
        assert!(schemas(r#""Missing""#, &[], limits()).is_err());
        assert!(schemas(r#""int""#, &[r#""int""#.into()], limits()).is_err());
        let definition = r#"{"type":"record","name":"R","fields":[]}"#.to_owned();
        assert!(schemas(r#""R""#, &[definition.clone(), definition], limits()).is_err());
    }

    #[test]
    fn namespace_expansion_cannot_multiply_schema_memory() {
        let namespace = "N".repeat(100_000);
        let fields: Vec<_> = (0..32)
            .map(|index| json!({"name":format!("v{index}"),"type":"R"}))
            .collect();
        let json =
            json!({"type":"record","name":"R","namespace":namespace,"fields":fields}).to_string();
        assert!(json.len() < MAX_BYTES);
        let error = schemas(&json, &[], limits()).unwrap_err();
        assert!(error.contains("normalized schema names"));
    }

    #[test]
    fn zero_size_fixed_and_object_form_primitives_are_supported() {
        assert_eq!(
            read(r#"{"type":"fixed","name":"Empty","size":0}"#, &[], &[]),
            Value::Fixed(0, vec![])
        );
        let mut limits = limits();
        limits.max_depth = 1;
        assert!(
            schemas(
                r#"{"type":"record","name":"R","fields":[{"name":"v","type":{"type":"int"}}]}"#,
                &[],
                limits
            )
            .is_ok()
        );
    }
}
