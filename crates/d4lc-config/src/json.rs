//! JSON reading and writing for the config files.
//!
//! Files are parsed with `serde_json` into a [`Value`] tree and read through
//! [`Fields`], which applies the lenient field rules the upstream-shaped
//! files need: a missing or mistyped field keeps its default, and integer
//! fields also accept numeric strings. Files are written through
//! [`Object`], which keeps the key order of the upstream entities, and
//! [`to_file_text`], which produces the indented layout upstream writes
//! (`WriteIndented = true`).

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

/// Parse `text` as a JSON object.
pub(crate) fn parse_object(text: &str, source: &str) -> Result<Map<String, Value>, String> {
    match serde_json::from_str(text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(format!("{source} must be a JSON object")),
        Err(error) => Err(format!("{source} is not valid JSON: {error}")),
    }
}

/// Parse `text` as a JSON array.
pub(crate) fn parse_array(text: &str, source: &str) -> Result<Vec<Value>, String> {
    match serde_json::from_str(text) {
        Ok(Value::Array(items)) => Ok(items),
        Ok(_) => Err(format!("{source} must be a JSON array")),
        Err(error) => Err(format!("{source} is not valid JSON: {error}")),
    }
}

/// The indented file text of `value`, with a trailing newline.
pub(crate) fn to_file_text(value: &impl Serialize) -> String {
    // Serializing a `Value` tree or an `Object` cannot fail: every key is a
    // string and no value carries a custom serializer.
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

/// `value` as a JSON number the way .NET writes a `double`: a whole number
/// without a fraction (`40`, not `40.0`).
pub(crate) fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9.0e15 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// Lenient typed access to the fields of one JSON object.
#[derive(Clone, Copy)]
pub(crate) struct Fields<'a> {
    object: &'a Map<String, Value>,
    source: &'a str,
}

impl<'a> Fields<'a> {
    pub(crate) fn new(object: &'a Map<String, Value>, source: &'a str) -> Self {
        Self { object, source }
    }

    /// The fields of `value` when it is an object.
    pub(crate) fn of(value: &'a Value, source: &'a str) -> Option<Self> {
        value.as_object().map(|object| Self::new(object, source))
    }

    pub(crate) fn value(&self, key: &str) -> Option<&'a Value> {
        self.object.get(key)
    }

    pub(crate) fn string(&self, key: &str) -> Option<String> {
        self.value(key)?.as_str().map(str::to_string)
    }

    /// A string field whose trimmed value is not empty.
    pub(crate) fn non_blank(&self, key: &str) -> Option<String> {
        self.string(key).filter(|value| !value.trim().is_empty())
    }

    pub(crate) fn bool(&self, key: &str) -> Option<bool> {
        self.value(key)?.as_bool()
    }

    /// A whole number, written as a JSON number or a numeric string.
    pub(crate) fn int(&self, key: &str) -> Option<i64> {
        match self.value(key)? {
            Value::Number(number) => number.as_i64().or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.fract() == 0.0 && value.abs() < 9.0e15)
                    .map(|value| value as i64)
            }),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// A number, written as a JSON number or a numeric string.
    pub(crate) fn float(&self, key: &str) -> Option<f64> {
        match self.value(key)? {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    pub(crate) fn i32(&self, key: &str) -> Result<Option<i32>, String> {
        self.ranged(key, "i32")
    }

    pub(crate) fn u32(&self, key: &str) -> Result<Option<u32>, String> {
        self.ranged(key, "0..=u32")
    }

    pub(crate) fn u8(&self, key: &str) -> Result<Option<u8>, String> {
        self.ranged(key, "0..=255")
    }

    fn ranged<T: TryFrom<i64>>(&self, key: &str, range: &str) -> Result<Option<T>, String> {
        let Some(value) = self.int(key) else {
            return Ok(None);
        };
        T::try_from(value)
            .map(Some)
            .map_err(|_| format!("{} {key} value {value} is outside {range}", self.source))
    }

    /// The object items of an array field; other items are skipped.
    pub(crate) fn objects(&self, key: &str) -> Option<Vec<Fields<'a>>> {
        let items = self.value(key)?.as_array()?;
        Some(
            items
                .iter()
                .filter_map(|item| Fields::of(item, self.source))
                .collect(),
        )
    }
}

/// A JSON object that serializes its fields in insertion order.
#[derive(Default)]
pub(crate) struct Object(Vec<(&'static str, Value)>);

impl Object {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn field(mut self, key: &'static str, value: impl Serialize) -> Self {
        // Plain data (strings, numbers, bools, vectors and maps of them)
        // always converts; a failure would be a programming error, so it is
        // written as `null` rather than aborting a save.
        self.0
            .push((key, serde_json::to_value(value).unwrap_or(Value::Null)));
        self
    }
}

impl Serialize for Object {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_fields_accept_numbers_and_numeric_strings() {
        let object = parse_object(
            r#"{"a": 5, "b": "7", "c": 5.0, "d": 5.5, "e": true, "f": -1}"#,
            "test",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let fields = Fields::new(&object, "test");
        assert_eq!(fields.int("a"), Some(5));
        assert_eq!(fields.int("b"), Some(7));
        assert_eq!(fields.int("c"), Some(5));
        assert_eq!(fields.int("d"), None);
        assert_eq!(fields.int("e"), None);
        assert_eq!(fields.int("missing"), None);
        assert_eq!(
            fields.u32("f"),
            Err("test f value -1 is outside 0..=u32".to_string())
        );
    }

    #[test]
    fn objects_keep_their_insertion_order() {
        let text = to_file_text(&Object::new().field("b", 1).field("a", "x"));
        assert_eq!(text, "{\n  \"b\": 1,\n  \"a\": \"x\"\n}\n");
    }
}
