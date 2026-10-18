//! Java `UUID.fromString` port for legacy item/entity/block-entity data (`< 2586`).
//!
//! Bytecode authority: local official 26.1.2 `server-26.1.2.jar`
//! (`.analysis/maps/falcons-rock/vanilla-hop/server/versions/26.1.2/server-26.1.2.jar`).
//! Receipts live in the ignored `.analysis/` directory (Mojang bytes never
//! enter Git):
//!
//! - `.analysis/codex-logs/old-dataversion/legacy-player-JavaUUID.javap`:
//!   `fromString` canonical 36-char fast path (dashes at UTF-16 offsets
//!   8/13/18/23, eight `parse4Nibbles` groups, any negative group falls
//!   through to `fromString1`), and `fromString1` short-form fallback
//!   (total UTF-16 `length() <= 36`, exactly four `-` separators found via
//!   chained `indexOf`, five `Long.parseLong` segments masked to
//!   `u32/u16/u16/u16/u48`, composed as
//!   `most = g0<<32|g1<<16|g2`, `least = g3<<48|g4`, no zero guard).
//! - `.analysis/codex-logs/old-dataversion/legacy-player-JavaLong.javap`:
//!   `Long.parseLong(CharSequence,int,int,int)` accepts one leading `-`/`+`
//!   per segment (negated when `-`), requires at least one digit afterwards,
//!   accumulates in the signed `i64` range (overflow throws), uses
//!   `Character.digit` per UTF-16 char, and never trims whitespace.
//! - `.analysis/codex-logs/old-dataversion/legacy-player-JavaCharacter.javap`:
//!   `Character.digit` dispatches to the Unicode `CharacterData` tables, so
//!   it is not ASCII-only. Bounded coverage here: ASCII hex plus the
//!   fullwidth mirrors (`０-９Ａ-Ｆａ-ｆ`); other Unicode digit blocks are
//!   rejected (`None`) so callers preserve the unknown string as-is.
//!
//! `old_items.rs` declares this module and re-exports [`parse_uuid`]; the
//! function is `pub(in crate::anvil::upgrade)` so the shared
//! item/entity/block-entity helpers can use it with no new crate
//! dependencies and no runtime Java.

/// Parse a legacy UUID string like Java `UUID.fromString`.
///
/// Returns the two UUID halves split into four big-endian `i32`s
/// `[most_hi, most_lo, least_hi, least_lo]`; `None` on oversize input,
/// wrong group count, empty group, bad digit, or signed overflow (callers
/// preserve the unknown string as-is). There is no least/most zero guard:
/// the nil UUID is valid. Digit coverage is ASCII hex plus fullwidth
/// mirrors; other Unicode digit blocks are rejected, see module docs.
pub(in crate::anvil::upgrade) fn parse_uuid(s: &str) -> Option<[i32; 4]> {
    // Java `fromString`: fast path only for UTF-16 length exactly 36.
    // Byte length 36 plus ASCII implies UTF-16 length 36.
    if s.len() == 36
        && s.is_ascii()
        && let Some(v) = canonical(s.as_bytes())
    {
        return Some(v);
    }
    fallback(s)
}

/// ASCII hex value, mirroring the `NIBBLES` table (`0-9A-Fa-f`, else `-1`).
fn ascii_hex(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'f' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'F' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }
}

/// Bounded `Character.digit(ch, 16)`: ASCII hex plus the fullwidth mirrors
/// (`U+FF10-FF19`, `U+FF21-FF26`, `U+FF41-FF46`). Other Unicode digit blocks
/// return `None` (caller preserves the string); see module docs.
fn java_digit(ch: char) -> Option<u32> {
    let v = ch as u32;
    match v {
        0x30..=0x39 => Some(v - 0x30),
        0x41..=0x46 => Some(v - 0x41 + 10),
        0x61..=0x66 => Some(v - 0x61 + 10),
        0xFF10..=0xFF19 => Some(v - 0xFF10),
        0xFF21..=0xFF26 => Some(v - 0xFF21 + 10),
        0xFF41..=0xFF46 => Some(v - 0xFF41 + 10),
        _ => None,
    }
}

/// Four hex chars starting at `off`, mirroring `parse4Nibbles`.
fn nibbles4(bytes: &[u8], off: usize) -> Option<u64> {
    let mut v: u64 = 0;
    for &b in bytes.get(off..off + 4)? {
        v = (v << 4) | u64::from(ascii_hex(b)?);
    }
    Some(v)
}

fn split(most: u64, least: u64) -> [i32; 4] {
    [
        (most >> 32) as u32 as i32,
        most as u32 as i32,
        (least >> 32) as u32 as i32,
        least as u32 as i32,
    ]
}

/// Canonical 36-char ASCII path (`parse4Nibbles` at 0/4/9/14/19/24/28/32).
fn canonical(bytes: &[u8]) -> Option<[i32; 4]> {
    if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
        return None;
    }
    let g = [
        nibbles4(bytes, 0)?,
        nibbles4(bytes, 4)?,
        nibbles4(bytes, 9)?,
        nibbles4(bytes, 14)?,
        nibbles4(bytes, 19)?,
        nibbles4(bytes, 24)?,
        nibbles4(bytes, 28)?,
        nibbles4(bytes, 32)?,
    ];
    let most = (g[0] << 48) | (g[1] << 32) | (g[2] << 16) | g[3];
    let least = (g[4] << 48) | (g[5] << 32) | (g[6] << 16) | g[7];
    Some(split(most, least))
}

/// One `Long.parseLong` segment with Java's mask applied, over a borrowed
/// slice (no temporary allocation). Mirrors the bytecode: single leading
/// `-`/`+`, at least one `Character.digit` char, signed-`i64` range.
fn parse_group(seg: &str, mask: u64) -> Option<u64> {
    let (negative, digits): (bool, &str) = match seg.as_bytes().first() {
        Some(b'+') => (false, &seg[1..]),
        Some(b'-') => (true, &seg[1..]),
        _ => (false, seg),
    };
    // Covers both the empty segment (`begin == end`) and the lone-sign
    // (`end - begin <= 1`) rejections in `parseLong`.
    if digits.is_empty() {
        return None;
    }
    let mut mag: u64 = 0;
    for c in digits.chars() {
        mag = mag
            .checked_mul(16)?
            .checked_add(u64::from(java_digit(c)?))?;
    }
    let raw: u64 = if negative {
        // `-` negates; `-i64::MIN` itself is in range.
        if mag > (i64::MAX as u64) + 1 {
            return None;
        }
        mag.wrapping_neg()
    } else {
        if mag > i64::MAX as u64 {
            return None;
        }
        mag
    };
    Some(raw & mask)
}

/// `fromString1`: total Java UTF-16 length `<= 36`, exactly four `-`
/// separators (five nonempty groups), groups masked to
/// `u32/u16/u16/u16/u48`. Byte-scanning for `-` is UTF-8 safe: no
/// multibyte sequence contains `0x2D`.
fn fallback(s: &str) -> Option<[i32; 4]> {
    // `String.length()` counts UTF-16 units. Byte length is always `>=` the
    // UTF-16 length, so only count units when bytes exceed 36.
    if s.len() > 36 && s.encode_utf16().count() > 36 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut dash = [0usize; 4];
    let mut count = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'-' {
            if count == 4 {
                return None; // Fifth dash: Java's `d6 >= 0` rejection.
            }
            dash[count] = i;
            count += 1;
        }
    }
    if count != 4 {
        return None; // Missing fourth dash: Java's `d5 < 0` rejection.
    }
    let g0 = parse_group(&s[..dash[0]], 0xFFFF_FFFF)?;
    let g1 = parse_group(&s[dash[0] + 1..dash[1]], 0xFFFF)?;
    let g2 = parse_group(&s[dash[1] + 1..dash[2]], 0xFFFF)?;
    let g3 = parse_group(&s[dash[2] + 1..dash[3]], 0xFFFF)?;
    let g4 = parse_group(&s[dash[3] + 1..], 0xFFFF_FFFF_FFFF)?;
    let most = (g0 << 32) | (g1 << 16) | g2;
    let least = (g3 << 48) | g4;
    Some(split(most, least))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_high_bits() {
        assert_eq!(
            parse_uuid("f81d4fae-7dec-11d0-a765-00a0c91e6bf6"),
            Some([
                0xF81D4FAE_u32 as i32,
                0x7DEC11D0_u32 as i32,
                0xA76500A0_u32 as i32,
                0xC91E6BF6_u32 as i32,
            ])
        );
        // Uppercase hex is canonical too (`NIBBLES` covers `A-F`).
        assert_eq!(
            parse_uuid("F81D4FAE-7DEC-11D0-A765-00A0C91E6BF6"),
            parse_uuid("f81d4fae-7dec-11d0-a765-00a0c91e6bf6")
        );
    }

    #[test]
    fn nil_is_valid_no_zero_guard() {
        assert_eq!(
            parse_uuid("00000000-0000-0000-0000-000000000000"),
            Some([0, 0, 0, 0])
        );
    }

    #[test]
    fn short_five_group_form() {
        // most = 0x00000001_00020003, least = 0x00040000_00000005.
        assert_eq!(parse_uuid("1-2-3-4-5"), Some([1, 131_075, 262_144, 5]));
    }

    #[test]
    fn leading_plus_per_group() {
        assert_eq!(parse_uuid("+1-+2-+3-+4-+5"), parse_uuid("1-2-3-4-5"));
    }

    #[test]
    fn leading_minus_adds_a_separator_and_is_rejected() {
        // A leading minus is an extra separator: reject before parsing groups.
        assert_eq!(parse_uuid("-1-2-3-4-5"), None);
        assert_eq!(parse_uuid("-8000000000000000-2-3-4-5"), None);
        assert_eq!(parse_uuid("-8000000000000001-2-3-4-5"), None);
    }

    #[test]
    fn signed_overflow_rejected() {
        // 2^63 does not fit the positive `parseLong` range.
        assert_eq!(parse_uuid("+8000000000000000-2-3-4-5"), None);
        // 17 hex digits exceed even `u64`, like Java's overflow throw.
        assert_eq!(parse_uuid("1-2-3-4-fffffffffffffffff"), None);
        // 16 f's exceed the signed `parseLong` range even though they fit
        // `u64`: positive overflow throws before the u48 mask applies.
        assert_eq!(parse_uuid("1-2-3-4-ffffffffffffffff"), None);
        // 12 f's are `u48::MAX` and fit `i64`: valid, unmasked.
        // least = 0x0004_FFFFFFFFFFFF -> hi 0x0004FFFF, lo 0xFFFFFFFF.
        assert_eq!(
            parse_uuid("1-2-3-4-ffffffffffff"),
            Some([1, 131_075, 327_679, -1])
        );
    }

    #[test]
    fn wrong_group_shapes_rejected() {
        assert_eq!(parse_uuid("1-2-3-4"), None); // four groups
        assert_eq!(parse_uuid("1-2-3-4-5-6"), None); // six groups
        assert_eq!(parse_uuid(""), None);
        assert_eq!(parse_uuid("1--3-4-5"), None); // empty group
        assert_eq!(parse_uuid("-2-3-4-5"), None); // leading empty group
        assert_eq!(parse_uuid("1-2-3-4-"), None); // trailing empty group
        assert_eq!(parse_uuid("+-2-3-4-5"), None); // lone sign group
    }

    #[test]
    fn dashless_32hex_rejected() {
        assert_eq!(parse_uuid("f81d4fae7dec11d0a76500a0c91e6bf6"), None);
    }

    #[test]
    fn no_trim_whitespace_rejected() {
        assert_eq!(parse_uuid(" 1-2-3-4-5"), None);
        assert_eq!(parse_uuid("1-2-3-4-5 "), None);
        assert_eq!(parse_uuid("1-2-3-4-5\n"), None);
    }

    #[test]
    fn bad_nibble_at_canonical_length_rejected() {
        // `g` is no hex digit in either the fast or the fallback path.
        assert_eq!(parse_uuid("f81d4fae-7dec-11d0-a765-00a0c91e6bg6"), None);
        // All-hex misplaced-dash input parses via `fromString1` masking
        // (g0 = 0xf81d4fae7 masked to u32), exactly like Java.
        assert_eq!(
            parse_uuid("f81d4fae7-dec-11d0-a765-00a0c91e6bf6"),
            Some([-2_116_748_569, 233_574_864, -1_486_552_928, -920_753_162])
        );
    }

    #[test]
    fn overlong_input_rejected() {
        // 37 ASCII units: Java's `length() > 36` throw.
        assert_eq!(parse_uuid("f81d4fae-7dec-11d0-a765-00a0c91e6bf66"), None);
    }

    #[test]
    fn java_utf16_length_not_bytes_or_chars() {
        // 19 astral chars = 19 Rust chars and 76 bytes, but 38 UTF-16
        // units, so Java rejects by length before parsing.
        let s = "\u{1D11E}".repeat(19);
        assert_eq!(s.chars().count(), 19);
        assert_eq!(parse_uuid(&s), None);
    }

    #[test]
    fn fullwidth_digits_accepted_like_character_data() {
        // `Character.digit` resolves fullwidth hex via the Unicode tables,
        // so the `fromString1` path accepts them.
        assert_eq!(parse_uuid("１-２-３-４-５"), parse_uuid("1-2-3-4-5"));
        // 12 fullwidth f's: 44 bytes but only 20 UTF-16 units (length OK),
        // value 0xFFFFFFFFFFFF fits u48 exactly: least = 0x0004FFFFFFFFFFFF.
        assert_eq!(
            parse_uuid("1-2-3-4-ｆｆｆｆｆｆｆｆｆｆｆｆ"),
            Some([1, 131_075, 327_679, -1])
        );
    }
}
