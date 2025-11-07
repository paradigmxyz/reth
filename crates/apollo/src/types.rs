use serde_json::Value as JsonValue;

/// Apollo-specific configuration that complements reth's config
#[derive(Debug, Clone)]
pub struct ApolloConfig {
    /// Apollo meta server URLs
    pub meta_server: Vec<String>,
    /// App ID in Apollo
    pub app_id: String,
    /// Cluster name (default: "default")
    pub cluster_name: String,
    /// Namespace (default: "application")
    pub namespaces: Option<Vec<String>>,
    /// Optional authentication token
    pub secret: Option<String>,
}

/// Apollo error enum
#[derive(Debug, thiserror::Error)]
pub enum ApolloError {
    /// Failed to initialize Apollo client
    #[error("Failed to initialize Apollo client: {0}")]
    ClientInit(String),
    /// Invalid namespace
    #[error("Invalid namespace: {0}")]
    InvalidNamespace(String),
}

/// Trait to convert from ConfigValue to concrete type
pub trait FromConfigValue: Sized {
    /// Convert from ConfigValue to concrete type
    fn try_from_config_value(value: &ConfigValue) -> Option<Self>;
}

impl FromConfigValue for u64 {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_u64()
    }
}

impl FromConfigValue for u32 {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_u32()
    }
}

impl FromConfigValue for i64 {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_i64()
    }
}

impl FromConfigValue for i32 {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_i32()
    }
}

impl FromConfigValue for f64 {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_f64()
    }
}

impl FromConfigValue for bool {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_bool()
    }
}

impl FromConfigValue for String {
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        value.try_as_string().map(|s| s.to_string())
    }
}

impl<T> FromConfigValue for Vec<T>
where
    T: FromConfigValue,
{
    fn try_from_config_value(value: &ConfigValue) -> Option<Self> {
        match value {
            ConfigValue::Array(values) => {
                values.iter().map(|v| T::try_from_config_value(v)).collect()
            }
            _ => None,
        }
    }
}

/// ConfigValue enum
#[derive(Debug, Clone)]
pub enum ConfigValue {
    /// 64-bit unsigned integer
    U64(u64),
    /// 32-bit unsigned integer
    U32(u32),
    /// 64-bit signed integer
    I64(i64),
    /// 32-bit signed integer
    I32(i32),
    /// Boolean
    Bool(bool),
    /// String
    String(String),
    /// 64-bit floating point number
    F64(f64),
    /// Array of config values
    Array(Vec<ConfigValue>),
}

/// ConfigValue implementation
impl ConfigValue {
    /// Parse from JsonValue once during cache update
    pub(crate) fn try_from_json(value: &JsonValue) -> Option<Self> {
        match value {
            JsonValue::Array(arr) => {
                let values: Vec<ConfigValue> =
                    arr.iter().filter_map(|v| ConfigValue::try_from_json(v)).collect();
                Some(ConfigValue::Array(values))
            }
            JsonValue::Number(n) => {
                if let Some(v) = n.as_u64() {
                    Some(ConfigValue::U64(v))
                } else if let Some(v) = n.as_i64() {
                    Some(ConfigValue::I64(v))
                } else if let Some(v) = n.as_f64() {
                    Some(ConfigValue::F64(v))
                } else {
                    None
                }
            }
            JsonValue::Bool(b) => Some(ConfigValue::Bool(*b)),
            JsonValue::String(s) => Some(ConfigValue::String(s.clone())),
            JsonValue::Null | JsonValue::Object(_) => None,
        }
    }

    fn try_as_u64(&self) -> Option<u64> {
        match self {
            ConfigValue::U64(v) => Some(*v),
            ConfigValue::U32(v) => Some(*v as u64),
            ConfigValue::I64(v) => (*v).try_into().ok(),
            ConfigValue::I32(v) => (*v).try_into().ok(),
            _ => None,
        }
    }

    fn try_as_u32(&self) -> Option<u32> {
        match self {
            ConfigValue::U32(v) => Some(*v),
            ConfigValue::U64(v) => (*v).try_into().ok(),
            ConfigValue::I64(v) => (*v).try_into().ok(),
            ConfigValue::I32(v) => (*v).try_into().ok(),
            _ => None,
        }
    }

    fn try_as_i64(&self) -> Option<i64> {
        match self {
            ConfigValue::I64(v) => Some(*v),
            ConfigValue::I32(v) => Some(*v as i64),
            ConfigValue::U64(v) => (*v).try_into().ok(),
            ConfigValue::U32(v) => Some(*v as i64),
            _ => None,
        }
    }

    fn try_as_i32(&self) -> Option<i32> {
        match self {
            ConfigValue::I32(v) => Some(*v),
            ConfigValue::I64(v) => (*v).try_into().ok(),
            ConfigValue::U64(v) => (*v).try_into().ok(),
            ConfigValue::U32(v) => (*v).try_into().ok(),
            _ => None,
        }
    }

    fn try_as_f64(&self) -> Option<f64> {
        match self {
            ConfigValue::F64(v) => Some(*v),
            _ => None,
        }
    }

    fn try_as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    fn try_as_string(&self) -> Option<&str> {
        match self {
            ConfigValue::String(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ===== ConfigValue::try_from_json tests =====

    #[test]
    fn test_from_json_primitives() {
        assert!(matches!(ConfigValue::try_from_json(&json!(195)), Some(ConfigValue::U64(195))));
        assert!(matches!(ConfigValue::try_from_json(&json!(-195)), Some(ConfigValue::I64(-195))));
        assert!(matches!(ConfigValue::try_from_json(&json!(true)), Some(ConfigValue::Bool(true))));
        assert!(matches!(
            ConfigValue::try_from_json(&json!(false)),
            Some(ConfigValue::Bool(false))
        ));
        assert!(
            matches!(ConfigValue::try_from_json(&json!(3.14)), Some(ConfigValue::F64(v)) if (v - 3.14).abs() < f64::EPSILON)
        );
        assert!(
            matches!(ConfigValue::try_from_json(&json!("hello")), Some(ConfigValue::String(s)) if s == "hello")
        );
    }

    #[test]
    fn test_from_json_arrays() {
        // Empty array
        assert!(
            matches!(ConfigValue::try_from_json(&json!([])), Some(ConfigValue::Array(v)) if v.is_empty())
        );

        // Simple array
        let result = ConfigValue::try_from_json(&json!([1, 2, 3]));
        assert!(matches!(result, Some(ConfigValue::Array(ref v)) if v.len() == 3));

        // Nested arrays
        let result = ConfigValue::try_from_json(&json!([[1, 2], [3, 4]]));
        assert!(matches!(result, Some(ConfigValue::Array(ref v)) if v.len() == 2));

        // Mixed types in array
        let result = ConfigValue::try_from_json(&json!([1, "foo", true]));
        assert!(matches!(result, Some(ConfigValue::Array(ref v)) if v.len() == 3));
    }

    #[test]
    fn test_from_json_null_and_unsupported() {
        // Null returns None
        assert!(ConfigValue::try_from_json(&json!(null)).is_none());

        // Objects return None (not supported)
        assert!(ConfigValue::try_from_json(&json!({"key": "value"})).is_none());

        // Array with nulls - nulls are filtered out
        let result = ConfigValue::try_from_json(&json!([1, null, 2]));
        assert!(matches!(result, Some(ConfigValue::Array(ref v)) if v.len() == 2));
    }

    // ===== try_as_u64 tests =====

    #[test]
    fn test_as_u64_same_type() {
        let val = ConfigValue::U64(195);
        assert_eq!(val.try_as_u64(), Some(195u64));

        let val = ConfigValue::U64(u64::MAX);
        assert_eq!(val.try_as_u64(), Some(u64::MAX as u64));
    }

    #[test]
    fn test_as_u64_from_u32() {
        let val = ConfigValue::U32(195);
        assert_eq!(val.try_as_u64(), Some(195u64));

        let val = ConfigValue::U32(u32::MAX);
        assert_eq!(val.try_as_u64(), Some(u32::MAX as u64));
    }

    #[test]
    fn test_as_u64_from_positive_signed() {
        let val = ConfigValue::I64(195);
        assert_eq!(val.try_as_u64(), Some(195u64));

        let val = ConfigValue::I32(195);
        assert_eq!(val.try_as_u64(), Some(195u64));

        let val = ConfigValue::I64(i64::MAX);
        assert_eq!(val.try_as_u64(), Some(i64::MAX as u64));
    }

    #[test]
    fn test_as_u64_from_negative_signed() {
        let val = ConfigValue::I64(-1);
        assert_eq!(val.try_as_u64(), None);

        let val = ConfigValue::I32(-1);
        assert_eq!(val.try_as_u64(), None);

        let val = ConfigValue::I64(i64::MIN);
        assert_eq!(val.try_as_u64(), None);
    }

    #[test]
    fn test_as_u64_from_invalid_types() {
        assert_eq!(ConfigValue::Bool(true).try_as_u64(), None);
        assert_eq!(ConfigValue::String("195".to_string()).try_as_u64(), None);
        assert_eq!(ConfigValue::F64(195.0).try_as_u64(), None);
        assert_eq!(ConfigValue::Array(vec![]).try_as_u64(), None);
    }

    // ===== try_as_u32 tests =====

    #[test]
    fn test_as_u32_same_type() {
        let val = ConfigValue::U32(195);
        assert_eq!(val.try_as_u32(), Some(195u32));

        let val = ConfigValue::U32(u32::MAX);
        assert_eq!(val.try_as_u32(), Some(u32::MAX));
    }

    #[test]
    fn test_as_u32_from_u64_in_range() {
        let val = ConfigValue::U64(195);
        assert_eq!(val.try_as_u32(), Some(195u32));

        let val = ConfigValue::U64(u32::MAX as u64);
        assert_eq!(val.try_as_u32(), Some(u32::MAX as u32));
    }

    #[test]
    fn test_as_u32_from_signed() {
        // Positive values in range
        let val = ConfigValue::I32(195);
        assert_eq!(val.try_as_u32(), Some(195u32));

        let val = ConfigValue::I64(195);
        assert_eq!(val.try_as_u32(), Some(195u32));

        // i32::MAX fits in u32
        let val = ConfigValue::I32(i32::MAX);
        assert_eq!(val.try_as_u32(), Some(i32::MAX as u32));

        // Negative values
        let val = ConfigValue::I32(-1);
        assert_eq!(val.try_as_u32(), None);

        let val = ConfigValue::I64(-1);
        assert_eq!(val.try_as_u32(), None);
    }

    #[test]
    fn test_as_i64_same_type() {
        let val = ConfigValue::I64(195);
        assert_eq!(val.try_as_i64(), Some(195i64));

        let val = ConfigValue::I64(i64::MIN);
        assert_eq!(val.try_as_i64(), Some(i64::MIN as i64));

        let val = ConfigValue::I64(i64::MAX);
        assert_eq!(val.try_as_i64(), Some(i64::MAX as i64));
    }

    #[test]
    fn test_as_i64_from_i32() {
        let val = ConfigValue::I32(195);
        assert_eq!(val.try_as_i64(), Some(195i64));

        let val = ConfigValue::I32(-195);
        assert_eq!(val.try_as_i64(), Some(-195i64));

        let val = ConfigValue::I32(i32::MAX);
        assert_eq!(val.try_as_i64(), Some(i32::MAX as i64));

        let val = ConfigValue::I32(i32::MIN);
        assert_eq!(val.try_as_i64(), Some(i32::MIN as i64));
    }

    #[test]
    fn test_as_i64_from_u32() {
        let val = ConfigValue::U32(195);
        assert_eq!(val.try_as_i64(), Some(195i64));

        // u32::MAX always fits in i64
        let val = ConfigValue::U32(u32::MAX);
        assert_eq!(val.try_as_i64(), Some(u32::MAX as i64));
    }

    #[test]
    fn test_as_i64_from_u64_in_range() {
        let val = ConfigValue::U64(195);
        assert_eq!(val.try_as_i64(), Some(195i64));

        let val = ConfigValue::U64(i64::MAX as u64);
        assert_eq!(val.try_as_i64(), Some(i64::MAX as i64));
    }

    #[test]
    fn test_as_i32_same_type() {
        let val = ConfigValue::I32(195);
        assert_eq!(val.try_as_i32(), Some(195i32));

        let val = ConfigValue::I32(i32::MIN);
        assert_eq!(val.try_as_i32(), Some(i32::MIN as i32));

        let val = ConfigValue::I32(i32::MAX);
        assert_eq!(val.try_as_i32(), Some(i32::MAX as i32));
    }

    #[test]
    fn test_as_i32_from_i64_in_range() {
        let val = ConfigValue::I64(195);
        assert_eq!(val.try_as_i32(), Some(195i32));

        let val = ConfigValue::I64(-195);
        assert_eq!(val.try_as_i32(), Some(-195i32));

        let val = ConfigValue::I64(i32::MAX as i64);
        assert_eq!(val.try_as_i32(), Some(i32::MAX as i32));

        let val = ConfigValue::I64(i32::MIN as i64);
        assert_eq!(val.try_as_i32(), Some(i32::MIN as i32));
    }

    #[test]
    fn test_as_i32_from_u32_in_range() {
        let val = ConfigValue::U32(195);
        assert_eq!(val.try_as_i32(), Some(195i32));

        let val = ConfigValue::U32(i32::MAX as u32);
        assert_eq!(val.try_as_i32(), Some(i32::MAX as i32));
    }

    #[test]
    fn test_as_i32_from_u64() {
        let val = ConfigValue::U64(195);
        assert_eq!(val.try_as_i32(), Some(195i32));

        let val = ConfigValue::U64(u64::MAX);
        assert_eq!(val.try_as_i32(), None);
    }

    // ===== try_as_f64 tests =====

    #[test]
    fn test_as_f64_strict() {
        let val = ConfigValue::F64(3.14);
        assert_eq!(val.try_as_f64(), Some(3.14f64));

        let val = ConfigValue::F64(f64::MAX);
        assert_eq!(val.try_as_f64(), Some(f64::MAX as f64));

        // All other types return None
        assert_eq!(ConfigValue::U64(195).try_as_f64(), None);
        assert_eq!(ConfigValue::I64(195).try_as_f64(), None);
        assert_eq!(ConfigValue::U32(195).try_as_f64(), None);
        assert_eq!(ConfigValue::I32(195).try_as_f64(), None);
        assert_eq!(ConfigValue::Bool(true).try_as_f64(), None);
        assert_eq!(ConfigValue::String("3.14".to_string()).try_as_f64(), None);
        assert_eq!(ConfigValue::Array(vec![]).try_as_f64(), None);
    }

    // ===== try_as_bool tests =====

    #[test]
    fn test_as_bool_strict() {
        let val = ConfigValue::Bool(true);
        assert_eq!(val.try_as_bool(), Some(true));

        let val = ConfigValue::Bool(false);
        assert_eq!(val.try_as_bool(), Some(false));

        // All other types return None
        assert_eq!(ConfigValue::U64(1).try_as_bool(), None);
        assert_eq!(ConfigValue::I64(0).try_as_bool(), None);
        assert_eq!(ConfigValue::String("true".to_string()).try_as_bool(), None);
        assert_eq!(ConfigValue::F64(0.0).try_as_bool(), None);
        assert_eq!(ConfigValue::Array(vec![]).try_as_bool(), None);
    }

    // ===== try_as_string tests =====

    #[test]
    fn test_as_string_strict() {
        let val = ConfigValue::String("hello".to_string());
        assert_eq!(val.try_as_string(), Some("hello"));

        let val = ConfigValue::String("".to_string());
        assert_eq!(val.try_as_string(), Some(""));

        // All other types return None
        assert_eq!(ConfigValue::U64(195).try_as_string(), None);
        assert_eq!(ConfigValue::Bool(true).try_as_string(), None);
        assert_eq!(ConfigValue::F64(3.14).try_as_string(), None);
        assert_eq!(ConfigValue::Array(vec![]).try_as_string(), None);
    }

    // ===== FromConfigValue trait tests =====

    #[test]
    fn test_from_config_value_u64() {
        assert_eq!(u64::try_from_config_value(&ConfigValue::U64(195)), Some(195u64));
        assert_eq!(u64::try_from_config_value(&ConfigValue::U32(195)), Some(195u64));
        assert_eq!(u64::try_from_config_value(&ConfigValue::I64(195)), Some(195u64));
        assert_eq!(u64::try_from_config_value(&ConfigValue::I64(-1)), None);
        assert_eq!(u64::try_from_config_value(&ConfigValue::String("195".to_string())), None);
    }

    #[test]
    fn test_from_config_value_u32() {
        assert_eq!(u32::try_from_config_value(&ConfigValue::U32(195)), Some(195u32));
        assert_eq!(u32::try_from_config_value(&ConfigValue::U64(195)), Some(195u32));
        assert_eq!(u32::try_from_config_value(&ConfigValue::U64(u64::MAX)), None);
    }

    #[test]
    fn test_from_config_value_i64() {
        assert_eq!(i64::try_from_config_value(&ConfigValue::I64(195)), Some(195i64));
        assert_eq!(i64::try_from_config_value(&ConfigValue::I32(-195)), Some(-195i64));
        assert_eq!(
            i64::try_from_config_value(&ConfigValue::U64(i64::MAX as u64)),
            Some(i64::MAX as i64)
        );
        assert_eq!(i64::try_from_config_value(&ConfigValue::U64(u64::MAX)), None);
    }

    #[test]
    fn test_from_config_value_i32() {
        assert_eq!(i32::try_from_config_value(&ConfigValue::I32(195)), Some(195i32));
        assert_eq!(i32::try_from_config_value(&ConfigValue::I64(i64::MAX)), None);
    }

    #[test]
    fn test_from_config_value_f64() {
        assert_eq!(f64::try_from_config_value(&ConfigValue::F64(3.14)), Some(3.14f64));
        assert_eq!(f64::try_from_config_value(&ConfigValue::U64(195)), None);
    }

    #[test]
    fn test_from_config_value_bool() {
        assert_eq!(bool::try_from_config_value(&ConfigValue::Bool(true)), Some(true));
        assert_eq!(bool::try_from_config_value(&ConfigValue::U64(1)), None);
    }

    #[test]
    fn test_from_config_value_string() {
        assert_eq!(
            String::try_from_config_value(&ConfigValue::String("hello".to_string())),
            Some("hello".to_string())
        );
        assert_eq!(String::try_from_config_value(&ConfigValue::U64(195)), None);
    }

    #[test]
    fn test_from_config_value_vec_homogeneous() {
        let arr =
            ConfigValue::Array(vec![ConfigValue::U64(1), ConfigValue::U64(2), ConfigValue::U64(3)]);
        assert_eq!(Vec::<u64>::try_from_config_value(&arr), Some(vec![1u64, 2u64, 3u64]));
    }

    #[test]
    fn test_from_config_value_vec_empty() {
        let arr = ConfigValue::Array(vec![]);
        assert_eq!(Vec::<u64>::try_from_config_value(&arr), Some(vec![]));
    }

    #[test]
    fn test_from_config_value_vec_type_mismatch() {
        // Array with mixed types that can't all convert to u64
        let arr = ConfigValue::Array(vec![
            ConfigValue::U64(1),
            ConfigValue::String("not a number".to_string()),
            ConfigValue::U64(3),
        ]);
        // Should return None because not all elements can convert
        assert_eq!(Vec::<u64>::try_from_config_value(&arr), None);
    }

    #[test]
    fn test_from_config_value_vec_with_convertible_types() {
        // Array with different int types that can all convert to u64
        let arr =
            ConfigValue::Array(vec![ConfigValue::U64(1), ConfigValue::U32(2), ConfigValue::I64(3)]);
        assert_eq!(Vec::<u64>::try_from_config_value(&arr), Some(vec![1u64, 2u64, 3u64]));
    }

    #[test]
    fn test_from_config_value_vec_non_array() {
        // Non-array should return None
        assert_eq!(Vec::<u64>::try_from_config_value(&ConfigValue::U64(195)), None);
        assert_eq!(Vec::<u64>::try_from_config_value(&ConfigValue::String("[]".to_string())), None);
    }

    #[test]
    fn test_from_config_value_vec_nested() {
        // Test Vec<Vec<u64>>
        let inner1 = ConfigValue::Array(vec![ConfigValue::U64(1), ConfigValue::U64(2)]);
        let inner2 = ConfigValue::Array(vec![ConfigValue::U64(3), ConfigValue::U64(4)]);
        let outer = ConfigValue::Array(vec![inner1, inner2]);

        assert_eq!(
            Vec::<Vec<u64>>::try_from_config_value(&outer),
            Some(vec![vec![1u64, 2u64], vec![3u64, 4u64]])
        );
    }

    #[test]
    fn test_from_config_value_vec_string() {
        let arr = ConfigValue::Array(vec![
            ConfigValue::String("foo".to_string()),
            ConfigValue::String("bar".to_string()),
        ]);
        assert_eq!(
            Vec::<String>::try_from_config_value(&arr),
            Some(vec!["foo".to_string(), "bar".to_string()])
        );
    }
}
