//! Deterministic JSON encoding for hashes and integrity contracts.

use serde::Serialize;

/// Serialize a value with object keys sorted recursively at every depth.
///
/// The explicit sort keeps the wire bytes stable even if `serde_json` is
/// compiled with an insertion-ordered map implementation in another consumer.
pub fn canonical_json_bytes<T: Serialize + ?Sized>(
    value: &T,
) -> Result<Vec<u8>, serde_json::Error> {
    let mut value = serde_json::to_value(value)?;
    sort_value(&mut value);
    serde_json::to_vec(&value)
}

fn sort_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                sort_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            let mut entries = std::mem::take(map).into_iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, mut nested) in entries {
                sort_value(&mut nested);
                map.insert(key, nested);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursively_sorts_nested_object_keys() -> Result<(), serde_json::Error> {
        let forward: serde_json::Value =
            serde_json::from_str(r#"{"outer":{"alpha":1,"omega":2},"tail":3}"#)?;
        let reverse: serde_json::Value =
            serde_json::from_str(r#"{"tail":3,"outer":{"omega":2,"alpha":1}}"#)?;

        assert_eq!(
            canonical_json_bytes(&forward)?,
            canonical_json_bytes(&reverse)?
        );
        Ok(())
    }
}
