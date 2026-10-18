use mc_nbt::{ListTag, Tag};

use super::nbt::{compound, fields, get, get_mut, number, remove, rename, set, string, take};
use super::{NodeKind, UpgradeError, UpgradeReport};

mod json;

// V4290/V4292 schema templates define the recursive children. In particular,
// `with` and arbitrary mod fields are remainder data, not TEXT_COMPONENT nodes.
// The walker treats Text as terminal: parsing a converted literal again at4290
// would incorrectly interpret text such as "123" as another serialized JSON.
pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind != NodeKind::Text {
        return Ok(());
    }
    match version {
        4290 => {
            let source = string(tag).ok_or_else(|| {
                unsupported(
                    4290,
                    "UnflattenTextComponentFix",
                    "text",
                    "old text component is not a JSON string",
                )
            })?;
            let converted = json::parse(source, "text")?;
            validate_text(&converted, 4290, "UnflattenTextComponentFix", "text")?;
            *tag = converted;
            report.applied(4290, "UnflattenTextComponentFix");
        }
        4291 => {
            migrate_tree(tag, Pass::LegacyHover, report, "text")?;
            migrate_tree(tag, Pass::Flags, report, "text")?;
        }
        4292 => {
            migrate_tree(tag, Pass::Events, report, "text")?;
        }
        _ => {}
    }
    Ok(())
}

fn unsupported(version: i32, fix: &'static str, path: &str, reason: &str) -> UpgradeError {
    UpgradeError::unsupported(version, fix, path, reason)
}

fn narrow_integer(value: i64) -> Tag {
    if let Ok(v) = i8::try_from(value) {
        Tag::Byte(v)
    } else if let Ok(v) = i16::try_from(value) {
        Tag::Short(v)
    } else if let Ok(v) = i32::try_from(value) {
        Tag::Int(v)
    } else {
        Tag::Long(value)
    }
}

// Borrowed RawValue tokens retain the exact decimal spelling locally. Binary
// float rounding alone cannot determine BigDecimal's exact-integral test.
fn json_number(source: &str, path: &str) -> Result<Tag, UpgradeError> {
    let (mantissa, exponent) = match source.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let exponent = exponent.parse::<i32>().map_err(|_| {
                unsupported(
                    4290,
                    "UnflattenTextComponentFix",
                    path,
                    "decimal exponent exceeds Java BigDecimal's range",
                )
            })?;
            (mantissa, i64::from(exponent))
        }
        None => (source, 0),
    };
    let negative = mantissa.starts_with('-');
    let unsigned = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let fraction = unsigned.split_once('.').map_or(0, |(_, v)| v.len());
    let mut digits: String = unsigned.chars().filter(|c| *c != '.').collect();
    let scale = exponent - fraction as i64;
    if i32::try_from(-scale).is_err() {
        return Err(unsupported(
            4290,
            "UnflattenTextComponentFix",
            path,
            "decimal scale exceeds Java BigDecimal's range",
        ));
    }
    let exact_integer = if scale < 0 {
        let removed = scale.unsigned_abs();
        if removed > digits.len() as u64 {
            digits.bytes().all(|b| b == b'0')
        } else {
            let split = digits.len() - removed as usize;
            if digits.as_bytes()[split..].iter().all(|b| *b == b'0') {
                digits.truncate(split);
                true
            } else {
                false
            }
        }
    } else {
        true
    };
    if exact_integer {
        let significant = digits.trim_start_matches('0');
        if significant.is_empty() {
            return Ok(Tag::Byte(0));
        }
        // A signed long has at most19 decimal digits; never materialize an
        // exponent-sized zero suffix for numbers which cannot be integral NBT.
        let added = scale.max(0) as u64;
        if significant.len() as u64 + added <= 19 {
            let mut integer = String::with_capacity(20);
            if negative {
                integer.push('-');
            }
            integer.push_str(significant);
            for _ in 0..added {
                integer.push('0');
            }
            if let Ok(value) = integer.parse::<i64>() {
                return Ok(narrow_integer(value));
            }
        }
    }
    let value = source.parse::<f64>().map_err(|_| {
        unsupported(
            4290,
            "UnflattenTextComponentFix",
            path,
            "JSON number has no NBT numeric representation",
        )
    })?;
    if !value.is_finite() {
        return Err(unsupported(
            4290,
            "UnflattenTextComponentFix",
            path,
            "nonfinite decimal conversion is unsupported",
        ));
    }
    Ok(if f64::from(value as f32) == value {
        Tag::Float(value as f32)
    } else {
        Tag::Double(value)
    })
}

// 26.1.2 ListTag accepts heterogeneous logical lists. Its wire encoding uses
// compound {"":value} wrappers, also wrapping singleton empty-key compounds
// to avoid ambiguity on read. Our ListTag is wire-typed, so encode those here.
pub(super) fn wire_list(mut values: Vec<Tag>) -> Tag {
    let first = values.first().map_or(0, Tag::type_id);
    let element_type = if values.iter().all(|v| v.type_id() == first) {
        first
    } else {
        10
    };
    if element_type == 10 {
        for value in &mut values {
            let wrapper = fields(value).is_some_and(|v| v.len() == 1 && v[0].0.is_empty());
            if value.type_id() != 10 || wrapper {
                let value_inner = std::mem::replace(value, Tag::Compound(Vec::new()));
                *value = compound(vec![("", value_inner)]);
            }
        }
    }
    Tag::List(ListTag {
        element_type,
        elements: values,
    })
}

fn logical(value: &Tag, list_type: u8) -> &Tag {
    if list_type == 10
        && let Some(fields) = fields(value)
        && fields.len() == 1
        && fields[0].0.is_empty()
    {
        return &fields[0].1;
    }
    value
}

pub(super) fn logical_mut(value: &mut Tag, list_type: u8) -> &mut Tag {
    if list_type == 10
        && matches!(value, Tag::Compound(fields) if fields.len() == 1 && fields[0].0.is_empty())
    {
        let Tag::Compound(fields) = value else {
            unreachable!("wrapper compound")
        };
        return &mut fields[0].1;
    }
    value
}

fn validate_text(
    tag: &Tag,
    version: i32,
    fix: &'static str,
    path: &str,
) -> Result<(), UpgradeError> {
    match tag {
        Tag::String(_) => Ok(()),
        Tag::List(list) => {
            for child in &list.elements {
                validate_text(logical(child, list.element_type), version, fix, path)?;
            }
            Ok(())
        }
        Tag::Compound(_) => {
            if let Some(extra) = get(tag, "extra") {
                let Tag::List(list) = extra else {
                    return Err(unsupported(version, fix, path, "text.extra is not a list"));
                };
                for child in &list.elements {
                    validate_text(logical(child, list.element_type), version, fix, path)?;
                }
            }
            if let Some(separator) = get(tag, "separator") {
                validate_text(separator, version, fix, path)?;
            }
            if let Some(hover) = get(tag, "hoverEvent") {
                if fields(hover).is_none() {
                    return Err(unsupported(
                        version,
                        fix,
                        path,
                        "hoverEvent is not a compound",
                    ));
                }
                match get(hover, "action").and_then(string) {
                    Some("show_text") => {
                        if let Some(contents) = get(hover, "contents") {
                            validate_text(contents, version, fix, path)?;
                        }
                    }
                    Some("show_entity") => {
                        // This is the actual V4290 schema location, not a
                        // broad search through untyped contents or custom data.
                        if let Some(name) = get(hover, "name") {
                            validate_text(name, version, fix, path)?;
                        }
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        _ => Err(unsupported(
            version,
            fix,
            path,
            "typed text must be a string, list or compound",
        )),
    }
}

#[derive(Clone, Copy)]
enum Pass {
    LegacyHover,
    Flags,
    Events,
}

fn migrate_tree(
    tag: &mut Tag,
    pass: Pass,
    report: &mut UpgradeReport,
    path: &str,
) -> Result<(), UpgradeError> {
    let (version, fix) = match pass {
        Pass::LegacyHover => (4291, "LegacyHoverEventFix"),
        Pass::Flags => (4291, "TextComponentStringifiedFlagsFix"),
        Pass::Events => (4292, "TextComponentHoverAndClickEventFix"),
    };
    match tag {
        Tag::String(_) => return Ok(()),
        Tag::List(list) => {
            for child in &mut list.elements {
                migrate_tree(logical_mut(child, list.element_type), pass, report, path)?;
            }
            return Ok(());
        }
        Tag::Compound(_) => {}
        _ => {
            return Err(unsupported(
                version,
                fix,
                path,
                "typed text must be a string, list or compound",
            ));
        }
    }
    if let Some(extra) = get_mut(tag, "extra") {
        let Tag::List(list) = extra else {
            return Err(unsupported(version, fix, path, "text.extra is not a list"));
        };
        for child in &mut list.elements {
            migrate_tree(
                logical_mut(child, list.element_type),
                pass,
                report,
                &format!("{path}.extra"),
            )?;
        }
    }
    if let Some(separator) = get_mut(tag, "separator") {
        migrate_tree(separator, pass, report, &format!("{path}.separator"))?;
    }
    if let Some(hover) = get_mut(tag, "hoverEvent") {
        if fields(hover).is_none() {
            return Err(unsupported(
                version,
                fix,
                path,
                "hoverEvent is not a compound",
            ));
        }
        match get(hover, "action").and_then(string) {
            Some("show_text") => {
                if let Some(contents) = get_mut(hover, "contents") {
                    migrate_tree(
                        contents,
                        pass,
                        report,
                        &format!("{path}.hoverEvent.contents"),
                    )?;
                }
            }
            Some("show_entity") => {
                if let Some(name) = get_mut(hover, "name") {
                    migrate_tree(name, pass, report, &format!("{path}.hoverEvent.name"))?;
                }
            }
            _ => {}
        }
    }
    match pass {
        Pass::LegacyHover => legacy_hover(tag, report, path)?,
        Pass::Flags => string_flags(tag, report, path)?,
        Pass::Events => events(tag, report, path)?,
    }
    Ok(())
}

fn string_flags(tag: &mut Tag, report: &mut UpgradeReport, path: &str) -> Result<(), UpgradeError> {
    let mut changed = false;
    for key in [
        "bold",
        "italic",
        "underlined",
        "strikethrough",
        "obfuscated",
    ] {
        if let Some(value) = get_mut(tag, key) {
            if let Some(text) = string(value) {
                *value = Tag::Byte(i8::from(text.eq_ignore_ascii_case("true")));
                changed = true;
            } else if number(value).is_none() {
                return Err(unsupported(
                    4291,
                    "TextComponentStringifiedFlagsFix",
                    path,
                    "style flag is neither a string nor a numeric boolean",
                ));
            }
        }
    }
    if changed {
        report.applied(4291, "TextComponentStringifiedFlagsFix");
    }
    Ok(())
}

fn legacy_hover(tag: &mut Tag, report: &mut UpgradeReport, path: &str) -> Result<(), UpgradeError> {
    let Some(hover) = get_mut(tag, "hoverEvent") else {
        return Ok(());
    };
    if get(hover, "value").is_none() {
        return Ok(());
    }
    if get(hover, "action").and_then(string) == Some("show_text") {
        if let Some(value) = get(hover, "value") {
            validate_text(value, 4291, "LegacyHoverEventFix", path)?;
        }
        rename(hover, "value", "contents");
    } else {
        // Vanilla does not parse legacy SNBT show_item/show_entity payloads.
        // It displays their stable JSON serialization as an explicit tooltip.
        let mut encoded = String::new();
        stable_json(hover, &mut encoded, path)?;
        *hover = compound(vec![
            ("action", Tag::String("show_text".to_owned())),
            (
                "contents",
                compound(vec![(
                    "text",
                    Tag::String(format!("Legacy hoverEvent: {encoded}")),
                )]),
            ),
        ]);
    }
    report.applied(4291, "LegacyHoverEventFix");
    Ok(())
}

fn json_string(value: &str, output: &mut String) -> Result<(), UpgradeError> {
    let encoded = serde_json::to_string(value).map_err(|_| {
        unsupported(
            4291,
            "LegacyHoverEventFix",
            "hoverEvent",
            "cannot serialize legacy JSON string",
        )
    })?;
    // Gson JsonWriter escapes JS line separators even when HTML safety is off.
    output.push_str(
        &encoded
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029"),
    );
    Ok(())
}

fn stable_json(tag: &Tag, output: &mut String, path: &str) -> Result<(), UpgradeError> {
    match tag {
        Tag::String(value) => json_string(value, output)?,
        Tag::Byte(v) => output.push_str(&v.to_string()),
        Tag::Short(v) => output.push_str(&v.to_string()),
        Tag::Int(v) => output.push_str(&v.to_string()),
        Tag::Long(v) => output.push_str(&v.to_string()),
        Tag::Float(_) | Tag::Double(_) => {
            return Err(unsupported(
                4291,
                "LegacyHoverEventFix",
                path,
                "legacy placeholder floating-point serialization requires exact Java numeric formatting",
            ));
        }
        Tag::Compound(fields) => {
            let mut entries: Vec<_> = fields.iter().collect();
            entries.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            if entries.windows(2).any(|w| w[0].0 == w[1].0) {
                return Err(unsupported(
                    4291,
                    "LegacyHoverEventFix",
                    path,
                    "duplicate NBT keys have no unambiguous JSON object representation",
                ));
            }
            output.push('{');
            for (i, (key, value)) in entries.into_iter().enumerate() {
                if i != 0 {
                    output.push(',');
                }
                json_string(key, output)?;
                output.push(':');
                stable_json(value, output, path)?;
            }
            output.push('}');
        }
        Tag::List(list) => {
            output.push('[');
            for (i, value) in list.elements.iter().enumerate() {
                if i != 0 {
                    output.push(',');
                }
                stable_json(logical(value, list.element_type), output, path)?;
            }
            output.push(']');
        }
        Tag::ByteArray(values) => {
            stable_integer_array(values.iter().map(|v| i64::from(*v)), output)
        }
        Tag::IntArray(values) => stable_integer_array(values.iter().map(|v| i64::from(*v)), output),
        Tag::LongArray(values) => stable_integer_array(values.iter().copied(), output),
    }
    Ok(())
}

fn stable_integer_array(values: impl Iterator<Item = i64>, output: &mut String) {
    output.push('[');
    for (i, value) in values.enumerate() {
        if i != 0 {
            output.push(',');
        }
        output.push_str(&value.to_string());
    }
    output.push(']');
}

fn events(tag: &mut Tag, report: &mut UpgradeReport, path: &str) -> Result<(), UpgradeError> {
    let mut changed = false;
    if let Some(hover) = get_mut(tag, "hoverEvent") {
        hover_event(hover, path)?;
        rename(tag, "hoverEvent", "hover_event");
        changed = true;
    }
    if let Some(click) = get_mut(tag, "clickEvent") {
        click_event(click, path)?;
        rename(tag, "clickEvent", "click_event");
        changed = true;
    }
    if changed {
        report.applied(4292, "TextComponentHoverAndClickEventFix");
    }
    Ok(())
}

fn hover_event(hover: &mut Tag, path: &str) -> Result<(), UpgradeError> {
    const FIX: &str = "TextComponentHoverAndClickEventFix";
    if fields(hover).is_none() {
        return Err(unsupported(4292, FIX, path, "hoverEvent is not a compound"));
    }
    match get(hover, "action").and_then(string) {
        Some("show_text") => {
            if let Some(contents) = get(hover, "contents") {
                validate_text(contents, 4292, FIX, path)?;
            }
            rename(hover, "contents", "value");
        }
        Some("show_item") => {
            if get(hover, "contents").and_then(string).is_some() {
                rename(hover, "contents", "id");
            } else if let Some(mut contents) = take(hover, "contents") {
                let Some(contents_fields) = fields(&contents) else {
                    return Err(unsupported(
                        4292,
                        FIX,
                        path,
                        "show_item contents is neither a compound nor an item identifier",
                    ));
                };
                if contents_fields
                    .iter()
                    .any(|(key, _)| !["id", "count", "components"].contains(&key.as_str()))
                {
                    return Err(unsupported(
                        4292,
                        FIX,
                        path,
                        "flattening show_item would discard unknown contents fields",
                    ));
                }
                if get(&contents, "id").is_some_and(|value| string(value).is_none())
                    || get(&contents, "count").is_some_and(|value| number(value).is_none())
                    || get(&contents, "components").is_some_and(|value| fields(value).is_none())
                {
                    return Err(unsupported(
                        4292,
                        FIX,
                        path,
                        "show_item contents has malformed id, count or components fields",
                    ));
                }
                for key in ["id", "count", "components"] {
                    if let Some(value) = take(&mut contents, key) {
                        set(hover, key, value);
                    }
                }
            }
        }
        Some("show_entity") => {
            if let Some(mut contents) = take(hover, "contents") {
                let Some(contents_fields) = fields(&contents) else {
                    return Err(unsupported(
                        4292,
                        FIX,
                        path,
                        "show_entity contents is not a compound",
                    ));
                };
                if contents_fields
                    .iter()
                    .any(|(key, _)| !["id", "type", "name"].contains(&key.as_str()))
                {
                    return Err(unsupported(
                        4292,
                        FIX,
                        path,
                        "flattening show_entity would discard unknown contents fields",
                    ));
                }
                if get(&contents, "type").is_some_and(|value| string(value).is_none()) {
                    return Err(unsupported(
                        4292,
                        FIX,
                        path,
                        "show_entity contents.type is not an entity identifier",
                    ));
                }
                if let Some(name) = get(&contents, "name") {
                    validate_text(name, 4292, FIX, path)?;
                }
                for key in ["id", "type", "name"] {
                    if let Some(value) = take(&mut contents, key) {
                        set(hover, key, value);
                    }
                }
            }
            rename(hover, "id", "uuid");
            rename(hover, "type", "id");
        }
        // Unknown actions remain opaque, as in the vanilla switch default.
        _ => {}
    }
    Ok(())
}

fn click_event(click: &mut Tag, path: &str) -> Result<(), UpgradeError> {
    const FIX: &str = "TextComponentHoverAndClickEventFix";
    if fields(click).is_none() {
        return Err(unsupported(4292, FIX, path, "clickEvent is not a compound"));
    }
    match get(click, "action").and_then(string) {
        Some("open_url") => {
            let value = get(click, "value").and_then(string).unwrap_or("");
            supported_http_uri(value).map_err(|reason| unsupported(4292, FIX, path, reason))?;
            rename(click, "value", "url");
        }
        Some("open_file") => {
            if get(click, "value").and_then(string).is_none() {
                return Err(unsupported(
                    4292,
                    FIX,
                    path,
                    "open_file value is not a string",
                ));
            }
            rename(click, "value", "path");
        }
        Some("run_command" | "suggest_command") => {
            let value = get(click, "value").and_then(string).ok_or_else(|| {
                unsupported(4292, FIX, path, "command click-event value is not a string")
            })?;
            if value
                .chars()
                .any(|c| c == '\u{a7}' || c < ' ' || c == '\u{7f}')
            {
                return Err(unsupported(
                    4292,
                    FIX,
                    path,
                    "invalid chat characters would cause vanilla to drop this click event",
                ));
            }
            rename(click, "value", "command");
        }
        Some("change_page") => {
            let value = get(click, "value")
                .ok_or_else(|| unsupported(4292, FIX, path, "change_page has no page value"))?;
            let page = old_page(value).ok_or_else(|| {
                unsupported(
                    4292,
                    FIX,
                    path,
                    "change_page has an unsupported or malformed integer",
                )
            })?;
            remove(click, "value");
            set(click, "page", Tag::Int(page.max(1)));
        }
        _ => {}
    }
    Ok(())
}

fn old_page(value: &Tag) -> Option<i32> {
    match value {
        Tag::Byte(v) => Some(i32::from(*v)),
        Tag::Short(v) => Some(i32::from(*v)),
        Tag::Int(v) => Some(*v),
        Tag::Long(v) => Some(*v as i32),
        Tag::Float(v) => Some(*v as i32),
        Tag::Double(v) => Some(*v as i32),
        Tag::String(v) => v.parse().ok(),
        _ => None,
    }
}

// java.net.URI accepts more than ordinary URLs (opaque URIs, registry-based
// authorities, IPv6 and Unicode). Support the unambiguous ASCII HTTP subset;
// explicitly reject the rest rather than substitute a different URL parser.
fn supported_http_uri(value: &str) -> Result<(), &'static str> {
    let Some((scheme, rest)) = value.split_once(':') else {
        return Err("open_url has no URI scheme");
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("open_url scheme is not HTTP or HTTPS");
    }
    if rest.is_empty() || rest.starts_with('#') {
        return Err("open_url has an empty scheme-specific part");
    }
    if !value.is_ascii() {
        return Err("Unicode java.net.URI validation is unsupported");
    }
    if value.bytes().any(|b| {
        b <= b' '
            || b == 127
            || matches!(
                b,
                b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}' | b'[' | b']'
            )
    }) {
        return Err("open_url has invalid or unsupported URI characters");
    }
    if rest.bytes().filter(|b| *b == b'#').count() > 1 {
        return Err("open_url has multiple fragment delimiters");
    }
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err("open_url has a malformed percent escape");
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    if let Some(authority_rest) = rest.strip_prefix("//") {
        let authority = authority_rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.is_empty() {
            return Err("empty HTTP authority requires unsupported Java URI path disambiguation");
        }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'))
        {
            return Err("registry-based, user-info or escaped HTTP authority is unsupported");
        }
        if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit())) {
            return Err("non-numeric HTTP authority port is unsupported");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(tag: &mut Tag, version: i32) -> Result<UpgradeReport, UpgradeError> {
        let mut report = UpgradeReport::default();
        fix_node(tag, NodeKind::Text, version, &mut report)?;
        Ok(report)
    }

    fn json(source: &str) -> Tag {
        Tag::String(source.to_owned())
    }

    #[test]
    fn unflatten4290_exact_json_narrowing_and_opaque_fields() {
        let mut text = json(
            r#"{"text":"123","bold":true,"opaque":{"n":128,"f":0.5,"long":2147483648,"x":"{\"text\":\"leave\"}"}}"#,
        );
        run(&mut text, 4290).unwrap();
        assert_eq!(get(&text, "text").and_then(string), Some("123"));
        assert_eq!(get(&text, "bold"), Some(&Tag::Byte(1)));
        let opaque = get(&text, "opaque").unwrap();
        assert_eq!(get(opaque, "n"), Some(&Tag::Short(128)));
        assert_eq!(get(opaque, "f"), Some(&Tag::Float(0.5)));
        assert_eq!(get(opaque, "long"), Some(&Tag::Long(2_147_483_648)));
        assert_eq!(
            get(opaque, "x").and_then(string),
            Some("{\"text\":\"leave\"}")
        );
    }

    #[test]
    fn unflatten4290_decimal_tokens_use_exact_bigdecimal_integrality() {
        let mut text = json(
            r#"{"rounded":1.0000000000000000000001,"exact":9007199254740993.0,"min":-9223372036854775808.0,"beyond":18446744073709551616,"small":1e-400,"scaled":1.2800e2}"#,
        );
        run(&mut text, 4290).unwrap();
        assert_eq!(get(&text, "rounded"), Some(&Tag::Float(1.0)));
        assert_eq!(get(&text, "exact"), Some(&Tag::Long(9_007_199_254_740_993)));
        assert_eq!(get(&text, "min"), Some(&Tag::Long(i64::MIN)));
        assert_eq!(
            get(&text, "beyond"),
            Some(&Tag::Float(18_446_744_073_709_551_616.0))
        );
        assert_eq!(get(&text, "small"), Some(&Tag::Float(0.0)));
        assert_eq!(get(&text, "scaled"), Some(&Tag::Short(128)));
    }

    #[test]
    fn unflatten4290_heterogeneous_lists_use_vanilla_wire_wrappers() {
        let mut text = json(r#"["literal",{"text":"object"}]"#);
        run(&mut text, 4290).unwrap();
        let Tag::List(list) = &text else {
            panic!("text list")
        };
        assert_eq!(list.element_type, 10);
        assert_eq!(get(&list.elements[0], "").and_then(string), Some("literal"));
        assert_eq!(
            get(&list.elements[1], "text").and_then(string),
            Some("object")
        );
        let wrapped = wire_list(vec![compound(vec![("", Tag::String("opaque".to_owned()))])]);
        let Tag::List(list) = wrapped else {
            panic!("wrapper list")
        };
        assert!(get(get(&list.elements[0], "").unwrap(), "").is_some());
    }

    #[test]
    fn unflatten4290_rejects_lenient_null_and_invalid_typed_representations() {
        for source in [
            "{broken",
            "null",
            "1",
            "true",
            "{'text':'lenient'}",
            "{\"text\":null}",
            "{\"extra\":4}",
            "{\"n\":1e999}",
        ] {
            let mut tag = json(source);
            let original = tag.clone();
            assert!(matches!(
                run(&mut tag, 4290),
                Err(UpgradeError::UnsupportedField {
                    version: 4290,
                    fix: "UnflattenTextComponentFix",
                    ..
                })
            ));
            assert_eq!(tag, original);
        }
    }

    #[test]
    fn flags4291_recurse_only_schema_defined_text_nodes() {
        let mut text = json(
            r#"{"text":"a","bold":"TrUe","italic":"not_true","extra":["b",{"text":"c","underlined":"true"}],"separator":{"text":"s","obfuscated":"true"},"with":[{"text":"opaque","bold":"true"}],"custom":{"bold":"true"}}"#,
        );
        run(&mut text, 4290).unwrap();
        run(&mut text, 4291).unwrap();
        assert_eq!(get(&text, "bold"), Some(&Tag::Byte(1)));
        assert_eq!(get(&text, "italic"), Some(&Tag::Byte(0)));
        let Tag::List(extra) = get(&text, "extra").unwrap() else {
            panic!("extra list")
        };
        assert_eq!(get(&extra.elements[1], "underlined"), Some(&Tag::Byte(1)));
        assert_eq!(
            get(get(&text, "separator").unwrap(), "obfuscated"),
            Some(&Tag::Byte(1))
        );
        let Tag::List(with) = get(&text, "with").unwrap() else {
            panic!("with list")
        };
        assert_eq!(
            get(&with.elements[0], "bold").and_then(string),
            Some("true")
        );
        assert_eq!(
            get(get(&text, "custom").unwrap(), "bold").and_then(string),
            Some("true")
        );
        let mut bad = compound(vec![("bold", compound(Vec::new()))]);
        assert!(matches!(
            run(&mut bad, 4291),
            Err(UpgradeError::UnsupportedField {
                fix: "TextComponentStringifiedFlagsFix",
                ..
            })
        ));
    }

    #[test]
    fn legacy_hover4291_show_text_and_other_actions_exact_placeholder() {
        let mut text =
            json(r#"{"text":"a","hoverEvent":{"action":"show_text","value":{"text":"tip"}}}"#);
        run(&mut text, 4290).unwrap();
        run(&mut text, 4291).unwrap();
        assert_eq!(
            get(
                get(get(&text, "hoverEvent").unwrap(), "contents").unwrap(),
                "text"
            )
            .and_then(string),
            Some("tip")
        );
        let mut item =
            json(r#"{"text":"a","hoverEvent":{"value":"{id:stone}","action":"show_item"}}"#);
        run(&mut item, 4290).unwrap();
        run(&mut item, 4291).unwrap();
        assert_eq!(
            get(
                get(get(&item, "hoverEvent").unwrap(), "contents").unwrap(),
                "text"
            )
            .and_then(string),
            Some("Legacy hoverEvent: {\"action\":\"show_item\",\"value\":\"{id:stone}\"}")
        );
        let mut bad = compound(vec![(
            "hoverEvent",
            compound(vec![
                ("action", Tag::String("show_item".to_owned())),
                ("value", Tag::Float(0.1)),
            ]),
        )]);
        assert!(matches!(
            run(&mut bad, 4291),
            Err(UpgradeError::UnsupportedField {
                fix: "LegacyHoverEventFix",
                ..
            })
        ));
    }

    #[test]
    fn events4292_hover_actions_flatten_contents_with_exact_field_names() {
        for (source, expected, value) in [
            (
                r#"{"hoverEvent":{"action":"show_text","contents":"tip"}}"#,
                "value",
                "tip",
            ),
            (
                r#"{"hoverEvent":{"action":"show_item","contents":"minecraft:stone"}}"#,
                "id",
                "minecraft:stone",
            ),
            (
                r#"{"hoverEvent":{"action":"show_item","contents":{"id":"minecraft:stone","count":2}}}"#,
                "id",
                "minecraft:stone",
            ),
            (
                r#"{"hoverEvent":{"action":"show_entity","contents":{"id":"uuid","type":"minecraft:pig","name":"Pig"}}}"#,
                "uuid",
                "uuid",
            ),
        ] {
            let mut text = json(source);
            run(&mut text, 4290).unwrap();
            run(&mut text, 4292).unwrap();
            assert!(get(&text, "hoverEvent").is_none());
            let event = get(&text, "hover_event").unwrap();
            assert_eq!(get(event, expected).and_then(string), Some(value));
            assert!(get(event, "contents").is_none());
        }
    }

    #[test]
    fn events4292_click_actions_and_numeric_page_semantics() {
        for (action, value, expected) in [
            ("open_url", "HTTPS://example.org/a%20b?q=x#f", "url"),
            ("open_file", "/tmp/test", "path"),
            ("run_command", "/say hello", "command"),
            ("suggest_command", "/help", "command"),
            ("copy_to_clipboard", "unchanged", "value"),
        ] {
            let mut text = compound(vec![(
                "clickEvent",
                compound(vec![
                    ("action", Tag::String(action.to_owned())),
                    ("value", Tag::String(value.to_owned())),
                ]),
            )]);
            run(&mut text, 4292).unwrap();
            assert_eq!(
                get(get(&text, "click_event").unwrap(), expected).and_then(string),
                Some(value)
            );
        }
        for (old, expected) in [
            (Tag::String("-5".to_owned()), 1),
            (Tag::Double(3.9), 3),
            (Tag::Long(4_294_967_298), 2),
        ] {
            let mut text = compound(vec![(
                "clickEvent",
                compound(vec![
                    ("action", Tag::String("change_page".to_owned())),
                    ("value", old),
                ]),
            )]);
            run(&mut text, 4292).unwrap();
            assert_eq!(
                get(get(&text, "click_event").unwrap(), "page"),
                Some(&Tag::Int(expected))
            );
        }
    }

    #[test]
    fn events4292_fail_closed_instead_of_dropping_malformed_events() {
        for (action, value) in [
            ("open_url", "ftp://example.org"),
            ("open_url", "https://example.org/%GG"),
            ("open_url", "https://[::1]/"),
            ("run_command", "/say\nno"),
            ("suggest_command", "\u{a7}a"),
            ("change_page", "bad"),
        ] {
            let mut text = compound(vec![(
                "clickEvent",
                compound(vec![
                    ("action", Tag::String(action.to_owned())),
                    ("value", Tag::String(value.to_owned())),
                ]),
            )]);
            assert!(matches!(
                run(&mut text, 4292),
                Err(UpgradeError::UnsupportedField {
                    version: 4292,
                    fix: "TextComponentHoverAndClickEventFix",
                    ..
                })
            ));
        }
        let mut text = compound(vec![(
            "hoverEvent",
            compound(vec![
                ("action", Tag::String("show_item".to_owned())),
                ("contents", Tag::Int(1)),
            ]),
        )]);
        assert!(run(&mut text, 4292).is_err());
    }

    #[test]
    fn unknown_actions_and_opaque_strings_are_never_broadly_replaced() {
        let mut text = compound(vec![
            ("text", Tag::String("hoverEvent".to_owned())),
            (
                "hoverEvent",
                compound(vec![
                    ("action", Tag::String("mod:custom".to_owned())),
                    ("contents", Tag::String("clickEvent".to_owned())),
                ]),
            ),
            (
                "custom_data",
                compound(vec![("hoverEvent", Tag::String("keep".to_owned()))]),
            ),
        ]);
        run(&mut text, 4292).unwrap();
        assert_eq!(get(&text, "text").and_then(string), Some("hoverEvent"));
        assert_eq!(
            get(get(&text, "hover_event").unwrap(), "contents").and_then(string),
            Some("clickEvent")
        );
        assert_eq!(
            get(get(&text, "custom_data").unwrap(), "hoverEvent").and_then(string),
            Some("keep")
        );
    }

    #[test]
    fn legacy_hover_then_flags4291_revisits_newly_typed_contents() {
        let mut text = json(
            r#"{"hoverEvent":{"action":"show_text","value":{"text":"tip","bold":"true","hoverEvent":{"action":"show_text","value":"nested"}}}}"#,
        );
        run(&mut text, 4290).unwrap();
        run(&mut text, 4291).unwrap();
        let contents = get(get(&text, "hoverEvent").unwrap(), "contents").unwrap();
        assert_eq!(get(contents, "bold"), Some(&Tag::Byte(1)));
    }

    #[test]
    fn events4292_rejects_malformed_typed_hover_fields() {
        for (action, key, value) in [
            ("show_item", "id", Tag::Int(1)),
            ("show_item", "count", Tag::String("bad".to_owned())),
            ("show_item", "components", Tag::Int(1)),
            ("show_entity", "type", Tag::Int(1)),
            ("show_entity", "name", Tag::Int(1)),
        ] {
            let mut text = compound(vec![(
                "hoverEvent",
                compound(vec![
                    ("action", Tag::String(action.to_owned())),
                    ("contents", compound(vec![(key, value)])),
                ]),
            )]);
            assert!(matches!(
                run(&mut text, 4292),
                Err(UpgradeError::UnsupportedField { version: 4292, .. })
            ));
        }
    }

    #[test]
    fn event_flattening4292_rejects_discarding_unknown_contents() {
        for action in ["show_item", "show_entity"] {
            let mut text = compound(vec![(
                "hoverEvent",
                compound(vec![
                    ("action", Tag::String(action.to_owned())),
                    ("contents", compound(vec![("mod:opaque", Tag::Int(1))])),
                ]),
            )]);
            assert!(matches!(
                run(&mut text, 4292),
                Err(UpgradeError::UnsupportedField { version: 4292, .. })
            ));
        }
        for action in ["open_file", "run_command", "suggest_command"] {
            let mut text = compound(vec![(
                "clickEvent",
                compound(vec![
                    ("action", Tag::String(action.to_owned())),
                    ("value", Tag::Int(1)),
                ]),
            )]);
            assert!(run(&mut text, 4292).is_err());
        }
    }
}
