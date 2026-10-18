//! Bounded SNBT reader for native command compounds; grammar follows OLD.
use mc_nbt::{ListTag, Tag};

pub(crate) fn parse(raw: &str) -> Result<Tag, &'static str> {
    let mut parser = Parser {
        raw,
        offset: 0,
        depth: 0,
    };
    let value = parser.value()?;
    parser.space();
    if parser.offset != raw.len() {
        return Err("Trailing SNBT");
    }
    Ok(value)
}

/// Read one component value while leaving the patch's comma/closing bracket
/// to the item argument reader. This is the same bounded SNBT codec used by
/// summon and block commands, not a second command-only grammar.
pub(crate) fn parse_prefix(raw: &str) -> Result<(Tag, usize), &'static str> {
    let mut parser = Parser {
        raw,
        offset: 0,
        depth: 0,
    };
    let value = parser.value()?;
    parser.space();
    Ok((value, parser.offset))
}
struct Parser<'a> {
    raw: &'a str,
    offset: usize,
    depth: usize,
}
impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.raw[self.offset..].chars().next()
    }
    fn take(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }
    fn space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.take();
        }
    }
    fn expect(&mut self, ch: char) -> Result<(), &'static str> {
        self.space();
        if self.take() == Some(ch) {
            Ok(())
        } else {
            Err("Invalid SNBT punctuation")
        }
    }
    fn token(&mut self, key: bool) -> Result<String, &'static str> {
        self.space();
        if let Some(quote @ ('\'' | '"')) = self.peek() {
            self.take();
            let mut value = String::new();
            while let Some(ch) = self.take() {
                if ch == quote {
                    return Ok(value);
                }
                if ch == '\\' {
                    value.push(self.take().ok_or("Invalid escape")?);
                } else {
                    value.push(ch);
                }
            }
            return Err("Unterminated SNBT string");
        }
        let start = self.offset;
        while self.peek().is_some_and(|ch| {
            !ch.is_whitespace() && !matches!(ch, ',' | ']' | '}' | '=' | '|') && (!key || ch != ':')
        }) {
            self.take();
        }
        if self.offset == start {
            return Err("Missing SNBT value");
        }
        Ok(self.raw[start..self.offset].into())
    }
    fn value(&mut self) -> Result<Tag, &'static str> {
        self.space();
        if self.depth >= 64 {
            return Err("SNBT nesting too deep");
        }
        self.depth += 1;
        let value = match self.peek() {
            Some('{') => {
                self.take();
                let mut fields = Vec::new();
                self.space();
                while self.peek() != Some('}') {
                    let key = self.token(true)?;
                    if fields.iter().any(|(other, _)| other == &key) {
                        return Err("Duplicate SNBT key");
                    }
                    self.expect(':')?;
                    fields.push((key, self.value()?));
                    self.space();
                    if self.peek() != Some(',') {
                        break;
                    }
                    self.take();
                }
                self.expect('}')?;
                Tag::Compound(fields)
            }
            Some('[') => {
                self.take();
                self.space();
                let array = if ["B;", "I;", "L;"]
                    .iter()
                    .any(|prefix| self.raw[self.offset..].starts_with(prefix))
                {
                    let kind = self.take();
                    self.take();
                    kind
                } else {
                    None
                };
                let mut values = Vec::new();
                self.space();
                while self.peek() != Some(']') {
                    values.push(self.value()?);
                    self.space();
                    if self.peek() != Some(',') {
                        break;
                    }
                    self.take();
                }
                self.expect(']')?;
                match array {
                    Some('B') => Tag::ByteArray(
                        values
                            .into_iter()
                            .map(|v| {
                                if let Tag::Byte(v) = v {
                                    Ok(v)
                                } else {
                                    Err("Expected byte")
                                }
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    Some('I') => Tag::IntArray(
                        values
                            .into_iter()
                            .map(|v| {
                                if let Tag::Int(v) = v {
                                    Ok(v)
                                } else {
                                    Err("Expected int")
                                }
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    Some('L') => Tag::LongArray(
                        values
                            .into_iter()
                            .map(|v| {
                                if let Tag::Long(v) = v {
                                    Ok(v)
                                } else {
                                    Err("Expected long")
                                }
                            })
                            .collect::<Result<_, _>>()?,
                    ),
                    _ => {
                        let mut element_type = values.first().map(Tag::type_id).unwrap_or(0);
                        if values.iter().any(|v| v.type_id() != element_type) {
                            element_type = mc_nbt::tag_type::COMPOUND;
                        }
                        // NbtOps/ListTag's 26.1.2 heterogeneous-list escape,
                        // shared by component lore and native list payloads.
                        if element_type == mc_nbt::tag_type::COMPOUND {
                            for value in &mut values {
                                if !matches!(value, Tag::Compound(fields) if fields.len() != 1 || !fields[0].0.is_empty())
                                {
                                    let original =
                                        std::mem::replace(value, Tag::Compound(Vec::new()));
                                    *value = Tag::Compound(vec![(String::new(), original)]);
                                }
                            }
                        }
                        Tag::List(ListTag {
                            element_type,
                            elements: values,
                        })
                    }
                }
            }
            Some('\'' | '"') => Tag::String(self.token(false)?),
            Some(_) => {
                let token = self.token(false)?;
                let suffix = token
                    .as_bytes()
                    .last()
                    .copied()
                    .unwrap_or(0)
                    .to_ascii_lowercase();
                let base = token.get(..token.len().saturating_sub(1)).unwrap_or(&token);
                match suffix {
                    b'b' if base.parse::<i8>().is_ok() => Tag::Byte(base.parse().unwrap()),
                    b's' if base.parse::<i16>().is_ok() => Tag::Short(base.parse().unwrap()),
                    b'l' if base.parse::<i64>().is_ok() => Tag::Long(base.parse().unwrap()),
                    b'f' if base.parse::<f32>().is_ok() => Tag::Float(base.parse().unwrap()),
                    b'd' if base.parse::<f64>().is_ok() => Tag::Double(base.parse().unwrap()),
                    _ => {
                        if token == "true" {
                            Tag::Byte(1)
                        } else if token == "false" {
                            Tag::Byte(0)
                        } else if let Ok(v) = token.parse::<i32>() {
                            Tag::Int(v)
                        } else if let Ok(v) = token.parse::<f64>() {
                            Tag::Double(v)
                        } else {
                            Tag::String(token)
                        }
                    }
                }
            }
            None => return Err("Missing SNBT value"),
        };
        self.depth -= 1;
        Ok(value)
    }
}
