//! Borrowed JSON tokens for V4290's exact decimal-to-NBT conversion.
//!
//! `raw_value` adds Serde's raw-token API without changing serde_json::Number.
//! All JSON grammar and string decoding remain serde_json's responsibility.

use mc_nbt::Tag;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;

use super::{UpgradeError, json_number, unsupported, wire_list};

pub(super) fn parse(source: &str, path: &str) -> Result<Tag, UpgradeError> {
    let raw = serde_json::from_str::<&RawValue>(source).map_err(|_| invalid(path))?;
    to_nbt(raw, path, 0)
}

fn invalid(path: &str) -> UpgradeError {
    unsupported(
        4290,
        "UnflattenTextComponentFix",
        path,
        "malformed or lenient-only JSON is unsupported; refusing to invent a literal fallback",
    )
}

fn to_nbt(raw: &RawValue, path: &str, depth: u8) -> Result<Tag, UpgradeError> {
    let source = raw.get().trim();
    match source.as_bytes().first().copied() {
        Some(b'n') => Err(unsupported(
            4290,
            "UnflattenTextComponentFix",
            path,
            "JSON null converts to EndTag, which is not representable by this NBT API",
        )),
        Some(b't' | b'f') => serde_json::from_str::<bool>(source)
            .map(|value| Tag::Byte(i8::from(value)))
            .map_err(|_| invalid(path)),
        Some(b'"') => serde_json::from_str::<String>(source)
            .map(Tag::String)
            .map_err(|_| invalid(path)),
        Some(b'-' | b'0'..=b'9') => json_number(source, path),
        // RawValue can skip deeply nested source without materializing it.
        // Retain the original Value deserializer's bound for our recursive walk.
        Some(b'[' | b'{') if depth >= 127 => Err(unsupported(
            4290,
            "UnflattenTextComponentFix",
            path,
            "JSON recursion limit exceeded",
        )),
        Some(b'[') => {
            let values = serde_json::from_str::<Vec<&RawValue>>(source)
                .map_err(|_| invalid(path))?
                .into_iter()
                .map(|value| to_nbt(value, path, depth + 1))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(wire_list(values))
        }
        Some(b'{') => {
            let ObjectFields(values) = serde_json::from_str(source).map_err(|_| invalid(path))?;
            let values = values
                .into_iter()
                .map(|(key, value)| {
                    let child = to_nbt(value, &format!("{path}.{key}"), depth + 1)?;
                    Ok((key, child))
                })
                .collect::<Result<Vec<_>, UpgradeError>>()?;
            Ok(Tag::Compound(values))
        }
        _ => Err(invalid(path)),
    }
}

struct ObjectFields<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for ObjectFields<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = ObjectFields<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut fields: Vec<(String, &'de RawValue)> = Vec::new();
                while let Some((key, raw)) = map.next_entry::<String, &'de RawValue>()? {
                    // Gson replaces duplicate values without moving the key.
                    // Resolve duplicates before conversion: overwritten nulls
                    // or out-of-range numbers must not affect the final value.
                    if let Some((_, value)) = fields.iter_mut().find(|(name, _)| name == &key) {
                        *value = raw;
                    } else {
                        fields.push((key, raw));
                    }
                }
                Ok(ObjectFields(fields))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::value::RawValue;

    use super::parse;
    use crate::anvil::upgrade::nbt::{get, string};
    use mc_nbt::Tag;

    #[test]
    fn decimal_tokens_are_borrowed_without_rounding_or_normalization() {
        for token in [
            "9007199254740993",
            "-9223372036854775808",
            "1.000000000000000000000000000000001",
            "12800e-2",
            "-0.0E+001",
            "1e2147483648",
        ] {
            let number = serde_json::from_str::<&RawValue>(token).unwrap();
            assert_eq!(number.get(), token);
            assert_eq!(number.get().as_ptr(), token.as_ptr());
        }
        let tag = parse(
            r#"{"rounded":1.000000000000000000000000000000001,"long":9007199254740993}"#,
            "text",
        )
        .unwrap();
        assert_eq!(get(&tag, "rounded"), Some(&Tag::Float(1.0)));
        assert_eq!(get(&tag, "long"), Some(&Tag::Long(9_007_199_254_740_993)));
        // Unrelated serde_json numbers keep the default normalization behavior.
        let number = serde_json::from_str::<serde_json::Value>("1e2").unwrap();
        assert_eq!(number.to_string(), "100.0");
    }

    #[test]
    fn strings_use_standard_escapes_and_objects_keep_the_last_key_in_place() {
        let tag = parse(
            r#"{"text":null,"n":1e2147483648,"text":"\uD83D\uDE00 \"quoted\" \\ /","n":9007199254740993}"#,
            "text",
        )
        .unwrap();
        assert_eq!(
            get(&tag, "text").and_then(string),
            Some("😀 \"quoted\" \\ /")
        );
        assert_eq!(get(&tag, "n"), Some(&Tag::Long(9_007_199_254_740_993)));
        let Tag::Compound(fields) = tag else {
            panic!("object");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].0, "text");
        assert_eq!(fields[1].0, "n");
    }

    #[test]
    fn strict_grammar_is_delegated_to_the_json_deserializer() {
        for source in [
            "",
            "01",
            "-01",
            "+1",
            "-",
            ".1",
            "1.",
            "1e",
            "1e+",
            "1e-",
            "NaN",
            "truefalse",
            "[1,]",
            "{\"x\":1,}",
            "{x:1}",
            "{'x':1}",
            "[1 2]",
            "{\"x\" 1}",
            "{\"x\":}",
            "\"\\q\"",
            "\"\\uD800\"",
            "\"a\nb\"",
            "\"tail\\",
            "\"unterminated",
            "null null",
            "\u{a0}1",
            "/*comment*/1",
        ] {
            assert!(parse(source, "text").is_err(), "accepted {source:?}");
        }
        for source in ["true", "false", "0", "-0", "1E-2", " \r\n[]\t", "{}"] {
            assert!(parse(source, "text").is_ok(), "rejected {source:?}");
        }
    }

    #[test]
    fn nesting_limit_matches_the_default_json_deserializer() {
        for depth in [126, 127, 128, 129] {
            let source = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(
                parse(&source, "text").is_ok(),
                serde_json::from_str::<serde_json::Value>(&source).is_ok(),
                "depth {depth}"
            );
            let empty = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(
                parse(&empty, "text").is_ok(),
                serde_json::from_str::<serde_json::Value>(&empty).is_ok(),
                "empty depth {depth}"
            );
        }
    }
}
