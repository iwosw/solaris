//! Shared text-component helpers for the pre-2586 item/entity/block-entity DFU port.
//!
//! Bytecode authority is the local official 26.1.2 jar (read only via
//! `javap -p -c`, never executed); receipts live under
//! `.analysis/codex-logs/old-dataversion/`:
//! - `legacy-player-LegacyComponentDataFixUtils.javap`:
//!   exact `rewriteFromLenient` framing/wrapper and `createTextComponentJson` shape.
//! - `text-net.minecraft.util.GsonHelper.javap`: `toStableString` is a fresh
//!   `JsonWriter` (compact style, `htmlSafe=false`) driven by `writeValue` with
//!   `Comparator.naturalOrder()` object keys, i.e. Java `String.compareTo`.
//! - `text-net.minecraft.util.LenientJsonParser.javap` + `text-net.minecraft.util.StrictJsonParser.javap`:
//!   lenient parsing is Gson `JsonParser.parseString`, whose reader runs in
//!   `LENIENT` strictness (legacy-strict is auto-switched before the read).
//! - `text-gson-com_google_gson_stream_JsonReader.javap`: the full lenient
//!   tokenizer (comments, single quotes, unquoted names/values, `;`/`=`
//!   separators, escape table, number state machine, 255 nesting limit).
//! - `text-gson-com_google_gson_internal_bind_JsonElementTypeAdapter.javap`:
//!   tree read (stray array separators materialize `null`; last duplicate key
//!   wins) and write shape.
//! - `text-gson-JsonWriter.javap`: `REPLACEMENT_CHARS` table, `string()`
//!   (only `<0x80` table hits plus U+2028/U+2029), compact separators,
//!   verbatim `Number.toString()` emission.
//! - `text-gson-com_google_gson_JsonPrimitive.javap` (`getAsString`: strings
//!   decode, numbers render via `getAsNumber().toString()`, booleans render
//!   `"true"`/`"false"`) and
//!   `text-gson-com_google_gson_internal_LazilyParsedNumber.javap`
//!   (`toString` returns the raw lexeme, never normalized).
//!
//! `old_items.rs` owns the module wiring and re-exports these two functions for
//! the entity/block-entity ports; `isStrictlyValidJson` and
//! `extractTranslationString` are out of scope here.

use std::cmp::Ordering;

/// Wrap `text` as a `{"text":...}` component, exactly like
/// `LegacyComponentDataFixUtils.createTextComponentJson`.
///
/// The input is ALWAYS a literal, including JSON-looking strings: no sniffing,
/// no pre-validation. Serialization is `GsonHelper.toStableString` over a
/// single-entry object, i.e. compact `{"text":"<escaped>"}` with Gson's
/// non-HTML-safe string escaping.
pub(in crate::anvil::upgrade) fn create_text_component_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 11);
    out.push_str("{\"text\":");
    push_quoted(&mut out, text);
    out.push('}');
    out
}

/// Port of `LegacyComponentDataFixUtils.rewriteFromLenient`, byte for byte:
///
/// - empty input or exactly `"null"` yields the empty component;
/// - WITHOUT any trimming, only inputs framed as `"..."`, `{...}` or `[...]`
///   (raw first/last chars) reach the lenient parser; anything else wraps the
///   ORIGINAL input as a literal;
/// - a parsed primitive becomes text via `JsonPrimitive.getAsString`
///   semantics; a parsed object/array is re-emitted via `toStableString`;
/// - any `JsonParseException` wraps the ORIGINAL input as a literal.
pub(in crate::anvil::upgrade) fn rewrite_from_lenient(source: &str) -> String {
    if source.is_empty() || source == "null" {
        return create_text_component_json("");
    }
    // Framing reads the raw edge units; the framing alphabet is ASCII so byte
    // indexing is exact.
    let bytes = source.as_bytes();
    let framed = matches!(
        (bytes[0], bytes[bytes.len() - 1]),
        (b'"', b'"') | (b'{', b'}') | (b'[', b']')
    );
    if !framed {
        return create_text_component_json(source);
    }
    let parsed = match parse_lenient(source) {
        Some(value) => value,
        None => return create_text_component_json(source),
    };
    match parsed {
        Json::Str(text) => create_text_component_json(&text),
        // LazilyParsedNumber.toString: the raw lexeme, never normalized.
        Json::Num(lexeme) => create_text_component_json(&lexeme),
        Json::Bool(flag) => create_text_component_json(if flag { "true" } else { "false" }),
        // JsonNull (and any other non-primitive) travels the stable path,
        // rendering as `null`.
        Json::Null | Json::Arr(_) | Json::Obj(_) => stable_string(&parsed),
    }
}

/// Gson string escaping with `htmlSafe=false` (`JsonWriter.string` over
/// `REPLACEMENT_CHARS`): `"` `\` plus the `b f n r t` short forms, remaining
/// `<0x20` controls as lowercase `\u00xx`, U+2028/U+2029 escaped, everything
/// else (including `<>&='`, DEL and all non-ASCII) raw.
fn push_quoted(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parsed JSON tree. Numbers keep the source lexeme verbatim
/// (`LazilyParsedNumber`), so large/exotic spellings survive untouched.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

/// `LenientJsonParser.parse` + the `JsonParser.parseReader(Reader)` trailing
/// check: exactly one top-level value, then only whitespace/comments to EOF
/// ("Did not consume the entire document." otherwise). `None` is any
/// `JsonParseException`/`MalformedJsonException`/`IOException` the reader can
/// raise on these inputs.
fn parse_lenient(source: &str) -> Option<Json> {
    let chars: Vec<char> = source.chars().collect();
    let mut parser = Parser {
        chars: &chars,
        pos: 0,
    };
    let value = parser.parse_value(0)?;
    parser.skip_ws_comments()?;
    if parser.pos != parser.chars.len() {
        return None;
    }
    Some(value)
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

impl Parser<'_> {
    /// One `JsonReader.nextNonWhitespace` pass: blanks plus the lenient-only
    /// `//`, `/*...*/` and `#` comments. `None` is an unterminated block
    /// comment; plain EOF simply stops.
    fn skip_ws_comments(&mut self) -> Option<()> {
        loop {
            match self.chars.get(self.pos) {
                Some('\n') | Some(' ') | Some('\r') | Some('\t') => {
                    self.pos += 1;
                }
                Some('/') => match self.chars.get(self.pos + 1) {
                    Some('/') => {
                        self.pos += 2;
                        while let Some(&c) = self.chars.get(self.pos) {
                            if c == '\n' || c == '\r' {
                                break;
                            }
                            self.pos += 1;
                        }
                    }
                    Some('*') => {
                        self.pos += 2;
                        loop {
                            let c = *self.chars.get(self.pos)?;
                            self.pos += 1;
                            if c == '*' && self.chars.get(self.pos) == Some(&'/') {
                                self.pos += 1;
                                break;
                            }
                        }
                    }
                    // A lone `/` stays significant; value parsing rejects it.
                    _ => break,
                },
                // `#` line comments are lenient-only (`checkLenient`).
                Some('#') => {
                    self.pos += 1;
                    while let Some(&c) = self.chars.get(self.pos) {
                        if c == '\n' || c == '\r' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        Some(())
    }

    /// `JsonReader.isLiteral` in lenient mode: the 16 structural/blank
    /// characters are non-literal, everything else (including quotes) is.
    fn parse_value(&mut self, depth: usize) -> Option<Json> {
        self.skip_ws_comments()?;
        match self.chars.get(self.pos) {
            Some('"') => {
                self.pos += 1;
                Some(Json::Str(self.quoted('"')?))
            }
            // Single-quoted strings are lenient-only (`checkLenient` in doPeek).
            Some('\'') => {
                self.pos += 1;
                Some(Json::Str(self.quoted('\'')?))
            }
            Some('{') => self.parse_object(depth).map(Json::Obj),
            Some('[') => self.parse_array(depth).map(Json::Arr),
            Some(_) => {
                // doPeek order: keywords, then numbers, then unquoted.
                if let Some(end) = self.keyword_end("true") {
                    self.pos = end;
                    return Some(Json::Bool(true));
                }
                if let Some(end) = self.keyword_end("false") {
                    self.pos = end;
                    return Some(Json::Bool(false));
                }
                if let Some(end) = self.keyword_end("null") {
                    self.pos = end;
                    return Some(Json::Null);
                }
                if let Some(end) = self.scan_number() {
                    let lexeme: String = self.chars[self.pos..end].iter().collect();
                    self.pos = end;
                    return Some(Json::Num(lexeme));
                }
                // doPeek rejects non-literal value starts ("Expected value").
                if is_literal(self.chars[self.pos]) {
                    Some(Json::Str(self.unquoted()?))
                } else {
                    None
                }
            }
            None => None,
        }
    }

    fn parse_object(&mut self, depth: usize) -> Option<Vec<(String, Json)>> {
        // beginObject -> push(): at most 255 nested containers.
        if depth >= 255 {
            return None;
        }
        self.pos += 1;
        let mut entries: Vec<(String, Json)> = Vec::new();
        loop {
            self.skip_ws_comments()?;
            match self.chars.get(self.pos) {
                None => return None,
                Some('}') => {
                    self.pos += 1;
                    return Some(entries);
                }
                Some(_) => {
                    let name = self.parse_name()?;
                    self.skip_ws_comments()?;
                    // `:` or the lenient `=` (with optional `>`).
                    match self.chars.get(self.pos) {
                        Some(':') => {
                            self.pos += 1;
                        }
                        Some('=') => {
                            self.pos += 1;
                            if self.chars.get(self.pos) == Some(&'>') {
                                self.pos += 1;
                            }
                        }
                        // "Expected ':'".
                        _ => return None,
                    }
                    let value = self.parse_value(depth + 1)?;
                    // JsonObject.add: last value wins, first position kept
                    // (moot after the stable sort, but exact).
                    if let Some(slot) = entries.iter_mut().find(|(key, _)| *key == name) {
                        slot.1 = value;
                    } else {
                        entries.push((name, value));
                    }
                    self.skip_ws_comments()?;
                    match self.chars.get(self.pos) {
                        // Trailing `,`/`;` before `}` is lenient-legal.
                        Some(',') | Some(';') => {
                            self.pos += 1;
                        }
                        Some('}') => {
                            self.pos += 1;
                            return Some(entries);
                        }
                        // "Unterminated object".
                        _ => return None,
                    }
                }
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Option<Vec<Json>> {
        // beginArray -> push(): at most 255 nested containers.
        if depth >= 255 {
            return None;
        }
        self.pos += 1;
        let mut items: Vec<Json> = Vec::new();
        loop {
            self.skip_ws_comments()?;
            match self.chars.get(self.pos) {
                None => return None,
                Some(']') => {
                    self.pos += 1;
                    return Some(items);
                }
                // Lenient doPeek rewinds a stray separator onto PEEKED_NULL,
                // so each one materializes a null element (`[1,,2]` ->
                // `[1, null, 2]`); a lone trailing comma just ends the array.
                Some(',') | Some(';') => {
                    self.pos += 1;
                    items.push(Json::Null);
                }
                Some(_) => {
                    items.push(self.parse_value(depth + 1)?);
                    self.skip_ws_comments()?;
                    match self.chars.get(self.pos) {
                        Some(',') | Some(';') => {
                            self.pos += 1;
                        }
                        Some(']') => {
                            self.pos += 1;
                            return Some(items);
                        }
                        // "Unterminated array".
                        _ => return None,
                    }
                }
            }
        }
    }

    /// `nextName`: quoted (either quote style) or unquoted; delimiters fail.
    fn parse_name(&mut self) -> Option<String> {
        match self.chars.get(self.pos) {
            Some('"') => {
                self.pos += 1;
                self.quoted('"')
            }
            Some('\'') => {
                self.pos += 1;
                self.quoted('\'')
            }
            Some(_) => self.unquoted(),
            None => None,
        }
    }

    /// `nextQuotedValue`: escapes decoded, raw controls/newlines tolerated in
    /// lenient mode, EOF is "Unterminated string".
    fn quoted(&mut self, quote: char) -> Option<String> {
        let mut out = String::new();
        loop {
            let c = *self.chars.get(self.pos)?;
            if c == quote {
                self.pos += 1;
                return Some(out);
            }
            if c == '\\' {
                self.pos += 1;
                out.push(self.escape()?);
            } else {
                out.push(c);
                self.pos += 1;
            }
        }
    }

    /// `readEscapeCharacter` in lenient mode: `t b n r f`, the quote/slash
    /// identities, `\'` and an escaped real newline; strict-only rejections do
    /// not apply. Anything else (`\v`, `\0`, `\x`, octal, bad `\u`) fails.
    fn escape(&mut self) -> Option<char> {
        let c = *self.chars.get(self.pos)?;
        self.pos += 1;
        match c {
            '"' | '/' | '\\' => Some(c),
            '\'' => Some('\''),
            't' => Some('\t'),
            'b' => Some('\u{08}'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            'f' => Some('\u{0C}'),
            '\n' => Some('\n'),
            'u' => {
                if self.pos + 4 > self.chars.len() {
                    // "Unterminated escape sequence".
                    return None;
                }
                let mut unit: u32 = 0;
                for k in 0..4 {
                    // Non-hex digits are "Malformed Unicode escape".
                    unit = unit * 16 + self.chars[self.pos + k].to_digit(16)?;
                }
                self.pos += 4;
                // Java yields the raw UTF-16 unit, so lone surrogates pass
                // through there; a Rust String cannot hold them, hence FFFD.
                Some(char::from_u32(unit).unwrap_or('\u{FFFD}'))
            }
            _ => None,
        }
    }

    /// `peekKeyword`: case-insensitive match (lenient; strict is never active
    /// here) requiring a non-literal terminator unit. At exact EOF, or when a
    /// literal char follows, the keyword is rejected and the token falls
    /// through to number/unquoted handling.
    fn keyword_end(&self, keyword: &str) -> Option<usize> {
        let end = self.pos + keyword.len();
        let term = *self.chars.get(end)?;
        for (i, expected) in keyword.chars().enumerate() {
            let got = self.chars[self.pos + i];
            if got != expected && got != expected.to_ascii_uppercase() {
                return None;
            }
        }
        if is_literal(term) {
            return None;
        }
        Some(end)
    }

    /// `peekNumber` state machine, transcribed from the reader bytecode:
    /// optional `-`, no `+`, no leading zeros, optional fraction (digits
    /// required after `.`), optional exponent (digits required, optional
    /// sign). The accumulated value uses wrapping `i64` exactly like Java's
    /// silent long overflow; only the end state matters here because both the
    /// long (15) and lexeme (16) outcomes keep the raw spelling downstream.
    /// Tokens longer than the 1024-unit reader buffer are rejected as numbers
    /// (they surface as unquoted strings instead).
    fn scan_number(&self) -> Option<usize> {
        let mut i = self.pos;
        let len = self.chars.len();
        let mut state: u8 = 0;
        let mut value: i64 = 0;
        loop {
            if i >= len {
                break;
            }
            if i - self.pos == 1024 && i < len {
                return None;
            }
            let c = self.chars[i];
            match c {
                '+' => {
                    if state == 5 {
                        state = 6;
                    } else {
                        return None;
                    }
                }
                '-' => {
                    if state == 0 {
                        state = 1;
                    } else if state == 5 {
                        state = 6;
                    } else {
                        return None;
                    }
                }
                '.' => {
                    if state == 2 {
                        state = 3;
                    } else {
                        return None;
                    }
                }
                'e' | 'E' => {
                    if state == 2 || state == 4 {
                        state = 5;
                    } else {
                        return None;
                    }
                }
                '0'..='9' => {
                    let digit = (c as u8 - b'0') as i64;
                    match state {
                        0 | 1 => {
                            value = -digit;
                            state = 2;
                        }
                        2 => {
                            if value == 0 {
                                return None;
                            }
                            value = value.wrapping_mul(10).wrapping_sub(digit);
                        }
                        3 => state = 4,
                        5 | 6 => state = 7,
                        _ => {}
                    }
                }
                _ => {
                    if is_literal(c) {
                        return None;
                    }
                    break;
                }
            }
            i += 1;
        }
        match state {
            2 | 4 | 7 => Some(i),
            _ => None,
        }
    }

    /// `nextUnquotedValue`: stops at blanks, structural units and the
    /// lenient-gated `# / ; = \` openers. An empty run is not a value.
    fn unquoted(&mut self) -> Option<String> {
        let start = self.pos;
        while let Some(&c) = self.chars.get(self.pos) {
            if matches!(
                c,
                '\u{09}'
                    | '\u{0A}'
                    | '\u{0C}'
                    | '\u{0D}'
                    | ' '
                    | '#'
                    | ','
                    | '/'
                    | ':'
                    | ';'
                    | '='
                    | '['
                    | '\\'
                    | ']'
                    | '{'
                    | '}'
            ) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        Some(self.chars[start..self.pos].iter().collect())
    }
}

/// `JsonReader.isLiteral` under lenient strictness (the `checkLenient` gates
/// always pass, so all 16 listed units are non-literal).
fn is_literal(c: char) -> bool {
    !matches!(
        c,
        '\u{09}'
            | '\u{0A}'
            | '\u{0C}'
            | '\u{0D}'
            | ' '
            | '#'
            | ','
            | '/'
            | ':'
            | ';'
            | '='
            | '['
            | '\\'
            | ']'
            | '{'
            | '}'
    )
}

/// `GsonHelper.toStableString`: compact writer output with object keys in
/// `Comparator.naturalOrder()`.
fn stable_string(value: &Json) -> String {
    let mut out = String::new();
    write_stable(&mut out, value);
    out
}

/// `GsonHelper.writeValue` with a non-null comparator: nulls/booleans as
/// literals, numbers via verbatim `Number.toString()`, strings quoted,
/// arrays/objects compact with `,`/`:` separators and UTF-16-sorted keys.
fn write_stable(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(lexeme) => out.push_str(lexeme),
        Json::Str(text) => push_quoted(out, text),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_stable(out, item);
            }
            out.push(']');
        }
        Json::Obj(entries) => {
            let mut order: Vec<usize> = (0..entries.len()).collect();
            order.sort_by(|&a, &b| cmp_utf16(&entries[a].0, &entries[b].0));
            out.push('{');
            for (i, &idx) in order.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                push_quoted(out, &entries[idx].0);
                out.push(':');
                write_stable(out, &entries[idx].1);
            }
            out.push('}');
        }
    }
}

/// Java `String.compareTo`: lexicographic order over UTF-16 code units.
/// This differs from Rust `str` ordering for supplementary characters
/// (e.g. U+E000 sorts AFTER U+10000 here, since 0xE000 > lead 0xD800).
fn cmp_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::{create_text_component_json, rewrite_from_lenient};

    #[test]
    fn literal_wrap_keeps_json_looking_input_verbatim() {
        assert_eq!(
            create_text_component_json("{\"text\":\"hi\"}"),
            "{\"text\":\"{\\\"text\\\":\\\"hi\\\"}\"}"
        );
        assert_eq!(create_text_component_json(""), "{\"text\":\"\"}");
        // Framed JSON-looking names parse, then re-emit the stable form.
        assert_eq!(
            rewrite_from_lenient("{\"text\":\"hi\"}"),
            "{\"text\":\"hi\"}"
        );
        assert_eq!(rewrite_from_lenient("Steve"), "{\"text\":\"Steve\"}");
        // Bare numbers never reach the parser (no framing chars).
        assert_eq!(rewrite_from_lenient("123"), "{\"text\":\"123\"}");
    }

    #[test]
    fn gson_escaping_without_html_escapes() {
        assert_eq!(
            create_text_component_json("a\"b\\c"),
            "{\"text\":\"a\\\"b\\\\c\"}"
        );
        assert_eq!(
            create_text_component_json("x\u{08}y\u{0C}z\t\n\r"),
            "{\"text\":\"x\\by\\fz\\t\\n\\r\"}"
        );
        assert_eq!(
            create_text_component_json("\u{01}\u{1F}\u{7F}"),
            "{\"text\":\"\\u0001\\u001f\"}"
        );
        assert_eq!(
            create_text_component_json("\u{2028}\u{2029}"),
            "{\"text\":\"\\u2028\\u2029\"}"
        );
        // htmlSafe=false: < > & = ' pass through raw.
        assert_eq!(create_text_component_json("<>&='"), "{\"text\":\"<>&='\"}");
        // Non-ASCII passes through raw.
        assert_eq!(create_text_component_json("é☃𐀀"), "{\"text\":\"é☃𐀀\"}");
    }

    #[test]
    fn empty_and_null_without_any_trimming() {
        assert_eq!(rewrite_from_lenient(""), "{\"text\":\"\"}");
        assert_eq!(rewrite_from_lenient("null"), "{\"text\":\"\"}");
        // No trim before the framing checks: padding wraps intact.
        assert_eq!(rewrite_from_lenient("  "), "{\"text\":\"  \"}");
        assert_eq!(rewrite_from_lenient(" null"), "{\"text\":\" null\"}");
        assert_eq!(rewrite_from_lenient("null "), "{\"text\":\"null \"}");
        assert_eq!(
            rewrite_from_lenient("  {\"a\":1}"),
            "{\"text\":\"  {\\\"a\\\":1}\"}"
        );
        assert_eq!(
            rewrite_from_lenient("{\"a\":1}  "),
            "{\"text\":\"{\\\"a\\\":1}  \"}"
        );
    }

    #[test]
    fn strict_values_round_trip_through_stable_form() {
        assert_eq!(
            rewrite_from_lenient("{\"b\":2,\"a\":1}"),
            "{\"a\":1,\"b\":2}"
        );
        assert_eq!(
            rewrite_from_lenient("[1,true,null,\"x\"]"),
            "[1,true,null,\"x\"]"
        );
        assert_eq!(rewrite_from_lenient("\"abc\""), "{\"text\":\"abc\"}");
        assert_eq!(rewrite_from_lenient("\"a\\\"b\""), "{\"text\":\"a\\\"b\"}");
        assert_eq!(
            rewrite_from_lenient("\"\u{0041}\"A\""),
            "{\"text\":\"\\\"A\\\"A\\\"\"}"
        );
        // Quoted primitives decode to text via getAsString semantics.
        assert_eq!(rewrite_from_lenient("\"123\""), "{\"text\":\"123\"}");
        assert_eq!(rewrite_from_lenient("\"true\""), "{\"text\":\"true\"}");
        assert_eq!(rewrite_from_lenient("\"null\""), "{\"text\":\"null\"}");
        assert_eq!(rewrite_from_lenient("\"\""), "{\"text\":\"\"}");
        // Duplicate keys: last value wins.
        assert_eq!(rewrite_from_lenient("{\"a\":1,\"a\":2}"), "{\"a\":2}");
        assert_eq!(
            rewrite_from_lenient("{\"a\":{\"b\":[1,{\"c\":null}]}}"),
            "{\"a\":{\"b\":[1,{\"c\":null}]}}"
        );
    }

    #[test]
    fn lenient_extras_comments_quotes_names_separators() {
        assert_eq!(rewrite_from_lenient("{a:1}"), "{\"a\":1}");
        assert_eq!(rewrite_from_lenient("{'a':'b',}"), "{\"a\":\"b\"}");
        assert_eq!(rewrite_from_lenient("['x',]"), "[\"x\"]");
        assert_eq!(rewrite_from_lenient("[1;2]"), "[1,2]");
        assert_eq!(rewrite_from_lenient("{\"a\"=1}"), "{\"a\":1}");
        assert_eq!(rewrite_from_lenient("{\"a\"=>1}"), "{\"a\":1}");
        assert_eq!(rewrite_from_lenient("{/*c*/\"a\":1//x\n}"), "{\"a\":1}");
        assert_eq!(rewrite_from_lenient("{#c\n\"a\":1}"), "{\"a\":1}");
        assert_eq!(rewrite_from_lenient("{a:'Aé'}"), "{\"a\":\"Aé\"}");
        assert_eq!(rewrite_from_lenient("{TRUE:False}"), "{\"TRUE\":false}");
        assert_eq!(rewrite_from_lenient("{'a\\'b':1}"), "{\"a'b\":1}");
    }

    #[test]
    fn stray_array_separators_materialize_nulls() {
        assert_eq!(rewrite_from_lenient("[,]"), "[null]");
        assert_eq!(rewrite_from_lenient("[1,,2]"), "[1,null,2]");
        assert_eq!(rewrite_from_lenient("[1,]"), "[1]");
        assert_eq!(rewrite_from_lenient("{\"a\":1,}"), "{\"a\":1}");
        // Object value slots have no such fallback: the original wraps.
        assert_eq!(
            rewrite_from_lenient("{\"a\":,}"),
            "{\"text\":\"{\\\"a\\\":,}\"}"
        );
    }

    #[test]
    fn number_lexemes_survive_and_keys_sort_by_utf16_units() {
        assert_eq!(
            rewrite_from_lenient("{\"b\":1,\"a\":12345678901234567890123456789}"),
            "{\"a\":12345678901234567890123456789,\"b\":1}"
        );
        assert_eq!(rewrite_from_lenient("{\"n\":1e400}"), "{\"n\":1e400}");
        assert_eq!(rewrite_from_lenient("{\"n\":-0.5E-3}"), "{\"n\":-0.5E-3}");
        let big = "9".repeat(200);
        assert_eq!(
            rewrite_from_lenient(&format!("{{\"n\":{big}}}")),
            format!("{{\"n\":{big}}}")
        );
        // The ASCII key "e000" sorts before a supplementary Unicode key.
        assert_eq!(
            rewrite_from_lenient("{\"e000\":1,\"𐀀\":2}"),
            "{\"e000\":1,\"𐀀\":2}"
        );
        // Actual U+E000 sorts after U+10000 in Java UTF-16 order.
        assert_eq!(
            rewrite_from_lenient("{\"\":1,\"𐀀\":2}"),
            "{\"𐀀\":2,\"\":1}"
        );
        // Malformed numbers are strings, not failures.
        assert_eq!(rewrite_from_lenient("{\"n\":01}"), "{\"n\":\"01\"}");
    }

    #[test]
    fn framed_garbage_wraps_the_original_input() {
        assert_eq!(rewrite_from_lenient("{oops}"), "{\"text\":\"{oops}\"}");
        assert_eq!(
            rewrite_from_lenient("{\"a\":}"),
            "{\"text\":\"{\\\"a\\\":}\"}"
        );
        assert_eq!(rewrite_from_lenient("{a:}"), "{\"text\":\"{a:}\"}");
        assert_eq!(rewrite_from_lenient("[\"x\""), "{\"text\":\"[\\\"x\\\"\"}");
        assert_eq!(rewrite_from_lenient("\"abc"), "{\"text\":\"\\\"abc\"}");
        assert_eq!(rewrite_from_lenient("{'a':1"), "{\"text\":\"{'a':1\"}");
        assert_eq!(
            rewrite_from_lenient("\"a\"\"b\""),
            "{\"text\":\"\\\"a\\\"\\\"b\\\"\"}"
        );
        assert_eq!(
            rewrite_from_lenient("{\"a\":1} trailing"),
            "{\"text\":\"{\\\"a\\\":1} trailing\"}"
        );
    }
}
