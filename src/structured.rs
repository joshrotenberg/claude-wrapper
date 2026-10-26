//! Typed decoding of `--json-schema` structured output.
//!
//! When a query or duplex session runs with a JSON schema
//! ([`QueryCommand::json_schema`](crate::QueryCommand::json_schema)), the
//! CLI validates the model's answer against it and reports the object as
//! `structured_output` on the terminal result. The schema is enforced by
//! the CLI; this crate does not validate the object against it again.
//! Decoding into a caller-chosen [`DeserializeOwned`] type is the
//! caller-side check that the value has the expected domain shape.
//!
//! [`QueryResult::decode_structured`](crate::types::QueryResult::decode_structured)
//! and `TurnResult::decode_structured` (duplex) apply the same rules, in
//! order:
//!
//! 1. A result with `is_error: true` is [`StructuredOutputError::NativeError`],
//!    even when it carries a partial `structured_output`.
//! 2. An absent or `null` `structured_output` is
//!    [`StructuredOutputError::Missing`].
//! 3. A value that is neither a JSON object nor an array is
//!    [`StructuredOutputError::NotObjectOrArray`].
//! 4. A value that does not deserialize into `T` is
//!    [`StructuredOutputError::Decode`].
//!
//! Process failures (non-zero exit, unparseable output, timeouts, rail
//! stops) are reported by the execution call as [`crate::Error`] before any
//! decoding happens. Decoding never starts another process.

use serde::de::DeserializeOwned;
use serde_json::Value;

/// Why a result's structured output could not be decoded.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StructuredOutputError {
    /// The native result was an error (`is_error: true`). `subtype` is the
    /// native subtype, e.g. `"error_max_structured_output_retries"`.
    #[error("the native result was an error (subtype: {subtype:?})")]
    NativeError {
        /// The result's `subtype` field, when present.
        subtype: Option<String>,
    },
    /// The result carried no `structured_output`, or it was `null`: no JSON
    /// schema took effect.
    #[error("the result carried no structured_output (no JSON schema took effect)")]
    Missing,
    /// `structured_output` was present but was neither an object nor an array.
    #[error("structured_output was neither a JSON object nor an array")]
    NotObjectOrArray,
    /// `structured_output` did not deserialize into the requested type.
    #[error("structured_output did not match the requested type: {0}")]
    Decode(#[source] serde_json::Error),
}

/// The object or array in `output`, or `None` when it is absent, null or a
/// scalar.
pub(crate) fn object_or_array(output: Option<&Value>) -> Option<&Value> {
    output.filter(|value| value.is_object() || value.is_array())
}

/// Apply the decoding rules in the module docs.
pub(crate) fn decode<T: DeserializeOwned>(
    is_error: bool,
    subtype: Option<&str>,
    output: Option<&Value>,
) -> Result<T, StructuredOutputError> {
    if is_error {
        return Err(StructuredOutputError::NativeError {
            subtype: subtype.map(str::to_owned),
        });
    }

    match output {
        None | Some(Value::Null) => Err(StructuredOutputError::Missing),
        Some(value) if value.is_object() || value.is_array() => {
            T::deserialize(value).map_err(StructuredOutputError::Decode)
        }
        Some(_) => Err(StructuredOutputError::NotObjectOrArray),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        name: String,
    }

    #[test]
    fn decodes_objects_and_arrays() {
        let object = json!({"name": "Ada"});
        let person: Person = decode(false, Some("success"), Some(&object)).unwrap();
        assert_eq!(person, Person { name: "Ada".into() });

        let array = json!([{"name": "Ada"}, {"name": "Grace"}]);
        let people: Vec<Person> = decode(false, None, Some(&array)).unwrap();
        assert_eq!(people.len(), 2);
    }

    #[test]
    fn native_error_wins_over_partial_output() {
        let partial = json!({"name": "partial"});
        let err = decode::<Person>(
            true,
            Some("error_max_structured_output_retries"),
            Some(&partial),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            StructuredOutputError::NativeError { subtype: Some(ref s) }
                if s == "error_max_structured_output_retries"
        ));
    }

    #[test]
    fn absent_null_scalar_and_wrong_shape_are_distinct() {
        assert!(matches!(
            decode::<Person>(false, None, None),
            Err(StructuredOutputError::Missing)
        ));
        assert!(matches!(
            decode::<Person>(false, None, Some(&Value::Null)),
            Err(StructuredOutputError::Missing)
        ));
        assert!(matches!(
            decode::<Person>(false, None, Some(&json!("Ada"))),
            Err(StructuredOutputError::NotObjectOrArray)
        ));
        assert!(matches!(
            decode::<Person>(false, None, Some(&json!({"nope": 1}))),
            Err(StructuredOutputError::Decode(_))
        ));
    }
}
