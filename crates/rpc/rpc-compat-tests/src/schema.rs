//! Rust-native loading of `OpenRPC` result schemas.

use eyre::{eyre, Context, Result};
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// Result schemas keyed by JSON-RPC method name.
#[derive(Debug, Clone, Default)]
pub struct SchemaCatalog {
    schemas: BTreeMap<String, Value>,
}

impl SchemaCatalog {
    /// Loads a generated `openrpc.json`, or assembles equivalent result schemas from source YAML.
    pub fn load(repository_root: &Path) -> Result<Self> {
        let generated = repository_root.join("openrpc.json");
        let catalog = if generated.is_file() {
            Self::from_openrpc(&generated)?
        } else if repository_root.join("src").is_dir() {
            Self::from_sources(repository_root)?
        } else {
            Self::default()
        };
        catalog.validate_schemas()?;
        Ok(catalog)
    }

    /// Returns the number of methods with result schemas.
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Returns true when no method schemas were found.
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    /// Validates a successful result against a method's schema.
    pub fn validate(&self, method: &str, result: &Value) -> Result<()> {
        let schema = self
            .schemas
            .get(method)
            .ok_or_else(|| eyre!("no OpenRPC result schema found for {method}"))?;
        let validator = jsonschema::validator_for(schema)
            .wrap_err_with(|| format!("invalid OpenRPC result schema for {method}"))?;
        let errors =
            validator.iter_errors(result).map(|error| error.to_string()).collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(eyre!("result does not conform to {method}: {}", errors.join("; ")))
        }
    }

    fn validate_schemas(&self) -> Result<()> {
        for (method, schema) in &self.schemas {
            jsonschema::validator_for(schema)
                .wrap_err_with(|| format!("invalid OpenRPC result schema for {method}"))?;
        }
        Ok(())
    }

    fn from_openrpc(path: &Path) -> Result<Self> {
        let document: Value = serde_json::from_str(&fs::read_to_string(path)?)
            .wrap_err_with(|| format!("failed to parse {}", path.display()))?;
        let mut components = document
            .pointer("/components/schemas")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        rewrite_component_refs(&mut components);
        let mut schemas = BTreeMap::new();
        for method in document["methods"].as_array().into_iter().flatten() {
            if let (Some(name), Some(schema)) =
                (method["name"].as_str(), method.pointer("/result/schema"))
            {
                let mut schema = schema.clone();
                rewrite_component_refs(&mut schema);
                let mut root_schema = Map::new();
                root_schema.insert(
                    "$schema".to_string(),
                    Value::String("https://json-schema.org/draft/2019-09/schema".to_string()),
                );
                root_schema.insert("$defs".to_string(), components.clone());
                if let Value::Object(result) = schema {
                    root_schema.extend(result);
                } else {
                    root_schema.insert("const".to_string(), schema);
                }
                schemas.insert(name.to_string(), Value::Object(root_schema));
            }
        }
        Ok(Self { schemas })
    }

    fn from_sources(root: &Path) -> Result<Self> {
        let schema_dirs = [root.join("src/schemas"), root.join("src/engine/openrpc/schemas")];
        let method_dirs = [
            root.join("src/eth"),
            root.join("src/debug"),
            root.join("src/txpool"),
            root.join("src/testing"),
            root.join("src/engine/openrpc/methods"),
        ];
        let mut components = Map::new();
        for path in source_files(&schema_dirs) {
            let document = read_document(&path)?;
            merge_components(&mut components, &document);
        }

        let mut schemas = BTreeMap::new();
        for path in source_files(&method_dirs) {
            let document = read_document(&path)?;
            let methods = document
                .as_array()
                .cloned()
                .or_else(|| document.get("methods").and_then(Value::as_array).cloned())
                .unwrap_or_default();
            for method in methods {
                let Some(name) = method.get("name").and_then(Value::as_str) else { continue };
                let Some(result) = method.pointer("/result/schema") else { continue };
                let mut schema = result.clone();
                rewrite_component_refs(&mut schema);
                let mut defs = Value::Object(components.clone());
                rewrite_component_refs(&mut defs);
                let mut root_schema = Map::new();
                root_schema.insert(
                    "$schema".to_string(),
                    Value::String("https://json-schema.org/draft/2019-09/schema".to_string()),
                );
                root_schema.insert("$defs".to_string(), defs);
                if let Value::Object(result) = schema {
                    root_schema.extend(result);
                } else {
                    root_schema.insert("const".to_string(), schema);
                }
                schemas.insert(name.to_string(), Value::Object(root_schema));
            }
        }
        if schemas.is_empty() {
            return Err(eyre!(
                "no OpenRPC source methods found below {}; provide openrpc.json or a full execution-apis checkout",
                root.display()
            ))
        }
        Ok(Self { schemas })
    }
}

fn source_files(directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = directories
        .iter()
        .filter(|directory| directory.is_dir())
        .flat_map(|directory| WalkDir::new(directory).into_iter().filter_map(|entry| entry.ok()))
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yaml" | "yml" | "json")
            )
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn read_document(path: &Path) -> Result<Value> {
    let contents = fs::read_to_string(path)?;
    if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
        serde_json::from_str(&contents).wrap_err_with(|| format!("invalid JSON {}", path.display()))
    } else {
        serde_yaml::from_str(&contents).wrap_err_with(|| format!("invalid YAML {}", path.display()))
    }
}

fn merge_components(target: &mut Map<String, Value>, document: &Value) {
    let object = document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .or_else(|| document.as_object());
    if let Some(object) = object {
        target.extend(object.iter().map(|(name, schema)| (name.clone(), schema.clone())));
    }
}

fn rewrite_component_refs(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") &&
                let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                *reference = format!("#/$defs/{name}");
            }
            object.values_mut().for_each(rewrite_component_refs);
        }
        Value::Array(values) => values.iter_mut().for_each(rewrite_component_refs),
        _ => {}
    }
}
