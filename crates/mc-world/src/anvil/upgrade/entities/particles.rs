//! ParticleUnflatteningFix3818, from the matching javap method bodies.
//! Numeric options use Brigadier's decimal grammar, not whitespace-normalized
//! or exponent/NaN parsing. Unsupported SNBT and malformed options fail closed.

use super::{UpgradeError, compound, fields, get, reject, rename, set, string, text};
use mc_nbt::{ListTag, Tag};

fn invalid(reason: &str) -> UpgradeError {
    reject(3818, "ParticleUnflatteningFix", "Particle", reason)
}

fn namespaced(value: &str) -> String {
    if value.contains(':') {
        value.to_owned()
    } else {
        format!("minecraft:{value}")
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b'_' | b'-' | b'.' | b'/' | b':')
        })
}

struct Reader<'a> {
    remaining: &'a str,
}

impl<'a> Reader<'a> {
    fn expect(&mut self, expected: char) -> Result<(), UpgradeError> {
        self.remaining = self
            .remaining
            .strip_prefix(expected)
            .ok_or_else(|| invalid("malformed particle option separator"))?;
        Ok(())
    }

    fn number(&mut self) -> Result<&'a str, UpgradeError> {
        let count = self
            .remaining
            .bytes()
            .take_while(|b| b.is_ascii_digit() || matches!(*b, b'-' | b'.'))
            .count();
        if count == 0 {
            return Err(invalid("missing decimal particle number"));
        }
        let (number, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        Ok(number)
    }

    fn float(&mut self) -> Result<f32, UpgradeError> {
        self.number()?
            .parse()
            .map_err(|_| invalid("invalid decimal particle float"))
    }

    fn double(&mut self) -> Result<f64, UpgradeError> {
        self.number()?
            .parse()
            .map_err(|_| invalid("invalid decimal particle double"))
    }

    fn int(&mut self) -> Result<i32, UpgradeError> {
        self.number()?
            .parse()
            .map_err(|_| invalid("invalid or overflowing particle integer"))
    }

    fn vector(&mut self) -> Result<Tag, UpgradeError> {
        let x = self.float()?;
        self.expect(' ')?;
        let y = self.float()?;
        self.expect(' ')?;
        let z = self.float()?;
        Ok(Tag::List(ListTag {
            element_type: 5,
            elements: vec![Tag::Float(x), Tag::Float(y), Tag::Float(z)],
        }))
    }

    fn finish(&self) -> Result<(), UpgradeError> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(invalid("unsupported trailing particle options"))
        }
    }

    fn whitespace(&mut self) {
        self.remaining = self.remaining.trim_start_matches(char::is_whitespace);
    }

    // StringReader.readString accepts single/double quotes and quote/backslash
    // escapes. Unquoted block-property strings use the Brigadier token alphabet.
    fn property_string(&mut self) -> Result<String, UpgradeError> {
        if let Some(quote @ ('\'' | '"')) = self.remaining.chars().next() {
            self.remaining = &self.remaining[1..];
            let mut value = String::new();
            let mut escaped = false;
            for (offset, ch) in self.remaining.char_indices() {
                if escaped {
                    if ch != quote && ch != '\\' {
                        return Err(invalid("unsupported block-property string escape"));
                    }
                    value.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == quote {
                    self.remaining = &self.remaining[offset + ch.len_utf8()..];
                    return Ok(value);
                } else {
                    value.push(ch);
                }
            }
            return Err(invalid("unterminated block-property string"));
        }
        let count = self
            .remaining
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'-' | b'.' | b'+'))
            .count();
        if count == 0 {
            return Err(invalid("missing block-property key or value"));
        }
        let (value, rest) = self.remaining.split_at(count);
        self.remaining = rest;
        Ok(value.to_owned())
    }
}

fn block(arguments: &str) -> Result<Tag, UpgradeError> {
    let (id, properties) = match arguments.split_once('[') {
        Some((id, properties)) => (id, Some(properties)),
        None => (arguments, None),
    };
    if !identifier(id) {
        return Err(invalid("unsupported block particle resource identifier"));
    }
    let mut state = compound(vec![("Name", Tag::String(namespaced(id)))]);
    if let Some(properties) = properties {
        let mut reader = Reader {
            remaining: properties,
        };
        reader.whitespace();
        let mut values = compound(vec![]);
        while !reader.remaining.starts_with(']') {
            let key = reader.property_string()?;
            reader.whitespace();
            reader.expect('=')?;
            reader.whitespace();
            let value = reader.property_string()?;
            set(&mut values, &key, Tag::String(value));
            reader.whitespace();
            if reader.remaining.starts_with(',') {
                reader.expect(',')?;
                reader.whitespace();
            } else {
                break;
            }
        }
        reader.expect(']')?;
        reader.finish()?;
        if fields(&values).is_some_and(|values| !values.is_empty()) {
            set(&mut state, "Properties", values);
        }
    }
    Ok(state)
}

fn item(arguments: &str) -> Result<Tag, UpgradeError> {
    // Vanilla accepts full TagParser SNBT. No available mc-nbt SNBT parser has
    // its semantics, so do not misparse or throw away embedded item metadata.
    if arguments.contains('{') {
        return Err(invalid("embedded particle item SNBT is not implemented"));
    }
    if !identifier(arguments) {
        return Err(invalid("unsupported item particle resource identifier"));
    }
    Ok(compound(vec![
        ("id", Tag::String(namespaced(arguments))),
        ("Count", Tag::Int(1)),
    ]))
}

fn floor(value: f32) -> i32 {
    // Mth.floor(F): Java float-to-int narrowing followed by wrapping subtraction.
    let narrowed = value as i32;
    if value < narrowed as f32 {
        narrowed.wrapping_sub(1)
    } else {
        narrowed
    }
}

pub(super) fn unflatten(value: &mut Tag) -> Result<bool, UpgradeError> {
    // Vanilla leaves a non-string Dynamic unchanged, including already structured
    // particles. The containing schema/walker retains its typed children.
    let Some(source) = string(value) else {
        return Ok(false);
    };
    let (kind, arguments) = source
        .split_once(' ')
        .map_or((source, None), |(kind, arguments)| (kind, Some(arguments)));
    let kind = namespaced(kind);
    let mut particle = compound(vec![]);
    if let Some(arguments) = arguments {
        match kind.as_str() {
            "minecraft:item" => set(&mut particle, "item", item(arguments)?),
            "minecraft:block"
            | "minecraft:block_marker"
            | "minecraft:falling_dust"
            | "minecraft:dust_pillar" => {
                set(&mut particle, "block_state", block(arguments)?);
            }
            "minecraft:dust" => {
                let mut reader = Reader {
                    remaining: arguments,
                };
                let color = reader.vector()?;
                reader.expect(' ')?;
                let scale = reader.float()?;
                reader.finish()?;
                set(&mut particle, "color", color);
                set(&mut particle, "scale", Tag::Float(scale));
            }
            "minecraft:dust_color_transition" => {
                let mut reader = Reader {
                    remaining: arguments,
                };
                let from = reader.vector()?;
                reader.expect(' ')?;
                let scale = reader.float()?;
                reader.expect(' ')?;
                let to = reader.vector()?;
                reader.finish()?;
                set(&mut particle, "from_color", from);
                set(&mut particle, "to_color", to);
                set(&mut particle, "scale", Tag::Float(scale));
            }
            "minecraft:sculk_charge" => {
                let mut reader = Reader {
                    remaining: arguments,
                };
                let roll = reader.float()?;
                reader.finish()?;
                set(&mut particle, "roll", Tag::Float(roll));
            }
            "minecraft:shriek" => {
                let mut reader = Reader {
                    remaining: arguments,
                };
                let delay = reader.int()?;
                reader.finish()?;
                set(&mut particle, "delay", Tag::Int(delay));
            }
            "minecraft:vibration" => {
                let mut reader = Reader {
                    remaining: arguments,
                };
                let x = reader.double()? as f32;
                reader.expect(' ')?;
                let y = reader.double()? as f32;
                reader.expect(' ')?;
                let z = reader.double()? as f32;
                reader.expect(' ')?;
                let ticks = reader.int()?;
                reader.finish()?;
                let destination = compound(vec![
                    ("type", text("minecraft:block")),
                    ("pos", Tag::IntArray(vec![floor(x), floor(y), floor(z)])),
                ]);
                set(&mut particle, "destination", destination);
                set(&mut particle, "arrival_in_ticks", Tag::Int(ticks));
            }
            _ => {
                if !arguments.is_empty() {
                    return Err(invalid("unknown simple particle has unsupported options"));
                }
            }
        }
    }
    set(&mut particle, "type", Tag::String(kind));
    *value = particle;
    Ok(true)
}

/// V4420 replaces the cloud's sole optional typed field `Particle` with
/// `custom_particle`. NamedEntityConvertUncheckedFix casts the same payload.
pub(super) fn custom_particle(tag: &mut Tag) -> Result<bool, UpgradeError> {
    if let Some(value) = get(tag, "Particle") {
        if fields(value).is_none() {
            return Err(reject(
                4420,
                "NamedEntityConvertUncheckedFix",
                "Particle",
                "legacy particle was not converted to a structured particle",
            ));
        }
        if get(tag, "custom_particle").is_some() {
            return Err(reject(
                4420,
                "NamedEntityConvertUncheckedFix",
                "Particle/custom_particle",
                "both old and new particle fields are present",
            ));
        }
        rename(tag, "Particle", "custom_particle");
        return Ok(true);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> Tag {
        let mut value = text(source);
        assert!(unflatten(&mut value).unwrap());
        value
    }

    #[test]
    fn simple_particles_and_already_structured_values() {
        for source in ["entity_effect", "minecraft:smoke", "mod:particle"] {
            let value = parse(source);
            assert_eq!(get(&value, "type"), Some(&Tag::String(namespaced(source))));
        }
        let mut value = compound(vec![
            ("type", text("minecraft:entity_effect")),
            ("custom", Tag::Int(7)),
        ]);
        let before = value.clone();
        assert!(!unflatten(&mut value).unwrap());
        assert_eq!(value, before);
    }

    #[test]
    fn block_particle_variants_and_quoted_properties() {
        for kind in ["block", "block_marker", "falling_dust", "dust_pillar"] {
            let value = parse(&format!("{kind} oak_log[axis=y, 'custom' = \"a\\\"b\"]"));
            let state = get(&value, "block_state").unwrap();
            assert_eq!(get(state, "Name"), Some(&text("minecraft:oak_log")));
            let properties = get(state, "Properties").unwrap();
            assert_eq!(get(properties, "axis"), Some(&text("y")));
            assert_eq!(get(properties, "custom"), Some(&text("a\"b")));
        }
        let value = parse("block minecraft:stone");
        assert!(get(get(&value, "block_state").unwrap(), "Properties").is_none());
    }

    #[test]
    fn bare_item_particle_stack() {
        let value = parse("item minecraft:stone");
        let item = get(&value, "item").unwrap();
        assert_eq!(get(item, "id"), Some(&text("minecraft:stone")));
        assert_eq!(get(item, "Count"), Some(&Tag::Int(1)));
    }

    #[test]
    fn dust_and_transition_vector_order() {
        let value = parse("dust 1 0 .5 2");
        assert_eq!(get(&value, "scale"), Some(&Tag::Float(2.0)));
        assert_eq!(
            get(&value, "color"),
            Some(&Tag::List(ListTag {
                element_type: 5,
                elements: vec![Tag::Float(1.0), Tag::Float(0.0), Tag::Float(0.5)]
            }))
        );
        let value = parse("dust_color_transition 1 0 0 1.5 0 0 1");
        assert_eq!(get(&value, "scale"), Some(&Tag::Float(1.5)));
        assert!(get(&value, "from_color").is_some());
        assert!(get(&value, "to_color").is_some());
    }

    #[test]
    fn sculk_shriek_and_vibration_primitive_options() {
        assert_eq!(
            get(&parse("sculk_charge -0.5"), "roll"),
            Some(&Tag::Float(-0.5))
        );
        assert_eq!(get(&parse("shriek 12"), "delay"), Some(&Tag::Int(12)));
        let value = parse("vibration -1.2 2.9 16777217 20");
        assert_eq!(get(&value, "arrival_in_ticks"), Some(&Tag::Int(20)));
        let destination = get(&value, "destination").unwrap();
        assert_eq!(get(destination, "type"), Some(&text("minecraft:block")));
        assert_eq!(
            get(destination, "pos"),
            Some(&Tag::IntArray(vec![-2, 2, 16777216]))
        );
    }

    #[test]
    fn unsupported_options_retain_original_and_emit_receipt() {
        for source in [
            "item stone{Damage:1}",
            "dust 1e0 0 0 1",
            "dust 1  0 0 1",
            "dust 1 0 0",
            "sculk_charge NaN",
            "shriek 2147483648",
            "vibration 1 2 3 1.5",
            "block stone[axis=]",
            "block stone[axis=x]extra",
            "mod:particle opaque",
        ] {
            let mut value = text(source);
            let before = value.clone();
            assert!(matches!(
                unflatten(&mut value).unwrap_err(),
                UpgradeError::UnsupportedField {
                    version: 3818,
                    fix: "ParticleUnflatteningFix",
                    ..
                }
            ));
            assert_eq!(value, before);
        }
    }

    #[test]
    fn cloud_schema_4420_cast_renames_typed_field_and_preserves_payload() {
        let particle = parse("entity_effect");
        let mut cloud = compound(vec![
            ("Particle", particle.clone()),
            ("opaque", Tag::Int(9)),
        ]);
        assert!(custom_particle(&mut cloud).unwrap());
        assert_eq!(get(&cloud, "custom_particle"), Some(&particle));
        assert_eq!(get(&cloud, "opaque"), Some(&Tag::Int(9)));
        assert!(get(&cloud, "Particle").is_none());
        let before = cloud.clone();
        assert!(!custom_particle(&mut cloud).unwrap());
        assert_eq!(cloud, before);
    }
}
