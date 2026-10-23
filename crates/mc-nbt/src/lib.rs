//! # mc-nbt
//!
//! Solaris' own NBT codec. The Named Binary Tag format used by Minecraft
//! comes in two flavours:
//!
//! - **Named NBT** (the original disk format): every tag carries its
//!   name. The root is `[type:u8][name_len:u16][name…][payload…]`.
//!   This is what region files and `.nbt` files on disk use.
//! - **Network NBT** (since 1.20.2): the root tag's name is stripped —
//!   `[type:u8][payload…]`. This is what packets like `Registry Data`
//!   and `Login (Play)` carry.
//!
//! Both flavours are exposed via [`read_named`] / [`write_named`] and
//! [`read_network`] / [`write_network`]; the payload format is shared.
//!
//! NBT strings use Java's Modified UTF-8: `NUL` is encoded as `C0 80`,
//! and supplementary Unicode characters are encoded as UTF-16 surrogate
//! pairs with three bytes per code unit.

use bytes::{Buf, BufMut};
use thiserror::Error;

// -----------------------------------------------------------------------
// Tag types
// -----------------------------------------------------------------------

/// Tag-type discriminator on the wire.
pub mod tag_type {
    pub const END: u8 = 0;
    pub const BYTE: u8 = 1;
    pub const SHORT: u8 = 2;
    pub const INT: u8 = 3;
    pub const LONG: u8 = 4;
    pub const FLOAT: u8 = 5;
    pub const DOUBLE: u8 = 6;
    pub const BYTE_ARRAY: u8 = 7;
    pub const STRING: u8 = 8;
    pub const LIST: u8 = 9;
    pub const COMPOUND: u8 = 10;
    pub const INT_ARRAY: u8 = 11;
    pub const LONG_ARRAY: u8 = 12;
}

/// A typed NBT value.
///
/// Compounds preserve insertion order — vanilla treats compound order
/// as ordered for hashing/equality purposes in some places, so emitting
/// them in the same order we read them is the safer default. Duplicate names
/// are preserved rather than collapsed; every occurrence counts toward parser
/// and writer budgets, and stricter duplicate semantics remain caller-owned.
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(ListTag),
    Compound(Vec<(String, Tag)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Tag {
    /// The on-wire tag-type byte for this value.
    #[must_use]
    pub fn type_id(&self) -> u8 {
        match self {
            Self::Byte(_) => tag_type::BYTE,
            Self::Short(_) => tag_type::SHORT,
            Self::Int(_) => tag_type::INT,
            Self::Long(_) => tag_type::LONG,
            Self::Float(_) => tag_type::FLOAT,
            Self::Double(_) => tag_type::DOUBLE,
            Self::ByteArray(_) => tag_type::BYTE_ARRAY,
            Self::String(_) => tag_type::STRING,
            Self::List(_) => tag_type::LIST,
            Self::Compound(_) => tag_type::COMPOUND,
            Self::IntArray(_) => tag_type::INT_ARRAY,
            Self::LongArray(_) => tag_type::LONG_ARRAY,
        }
    }
}

/// A homogenously-typed list. All elements share `element_type`; for an
/// empty list `element_type` is conventionally `End` (0) in vanilla.
#[derive(Debug, Clone, PartialEq)]
pub struct ListTag {
    pub element_type: u8,
    pub elements: Vec<Tag>,
}

impl ListTag {
    /// Empty list with the conventional `End` element type.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            element_type: tag_type::END,
            elements: Vec::new(),
        }
    }
}

// -----------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------

/// Hard ceiling for array payloads and list backing allocations.
/// Strings have their own `u16` wire-length limit.
pub const MAX_NBT_LENGTH: usize = 16 * 1024 * 1024;

/// Vanilla rejects container nesting beyond 512 levels.
pub const MAX_NBT_DEPTH: usize = 512;

/// Aggregate encoded bytes accepted or emitted for one NBT root.
pub const MAX_NBT_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Aggregate decoded tag-node ceiling for one NBT root.
pub const MAX_NBT_NODES: usize = 1_048_576;
/// Per-compound entry ceiling. Duplicate names are preserved and count separately.
pub const MAX_NBT_COMPOUND_ENTRIES: usize = 65_536;
/// Aggregate compound-entry ceiling across one NBT root.
pub const MAX_NBT_TOTAL_COMPOUND_ENTRIES: usize = 1_048_576;
/// Aggregate Modified UTF-8 payload bytes across names and string values.
pub const MAX_NBT_STRING_BYTES: usize = MAX_NBT_LENGTH;
/// Aggregate heap-allocation estimate for one decoded NBT root.
pub const MAX_NBT_ALLOCATION_BYTES: usize = 64 * 1024 * 1024;

const COMPOUND_RESERVE_CHUNK: usize = 64;
const FRAME_STACK_CAPACITY: usize = MAX_NBT_DEPTH * 2 + 8;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NbtError {
    #[error("ran out of NBT bytes (needed {needed} more, had {available})")]
    Underflow { needed: usize, available: usize },

    #[error("expected a root Compound tag, got tag-id {0}")]
    RootMustBeCompound(u8),

    #[error("unknown tag-id {0}")]
    UnknownTag(u8),

    #[error("NBT collection payload of {0} bytes exceeds the {MAX_NBT_LENGTH}-byte ceiling")]
    ArrayTooLong(i64),

    #[error("Modified UTF-8 string payload of {0} bytes exceeds the u16 wire-length limit")]
    StringTooLong(usize),

    #[error("negative NBT array length: {0}")]
    NegativeLength(i32),

    #[error("NBT container nesting exceeds the {MAX_NBT_DEPTH}-level limit")]
    NestingTooDeep,

    #[error("NBT {resource} budget exceeds the configured limit of {limit}")]
    BudgetExceeded {
        resource: &'static str,
        limit: usize,
    },

    #[error("failed to reserve {bytes} byte(s) for NBT {resource}")]
    AllocationFailed {
        resource: &'static str,
        bytes: usize,
    },

    #[error("string is not valid Modified UTF-8")]
    InvalidString,

    #[error("non-Compound type {0:#x} used where a Compound is required")]
    NotCompound(u8),

    #[error("list of {parent:#x}s contains an element of incompatible type {found:#x}")]
    HeterogeneousList { parent: u8, found: u8 },
}

#[derive(Debug, Clone, Copy)]
struct NbtLimits {
    wire_bytes: usize,
    nodes: usize,
    compound_entries: usize,
    string_bytes: usize,
    allocation_bytes: usize,
}

const PRODUCTION_LIMITS: NbtLimits = NbtLimits {
    wire_bytes: MAX_NBT_TOTAL_BYTES,
    nodes: MAX_NBT_NODES,
    compound_entries: MAX_NBT_TOTAL_COMPOUND_ENTRIES,
    string_bytes: MAX_NBT_STRING_BYTES,
    allocation_bytes: MAX_NBT_ALLOCATION_BYTES,
};

// Entity checkpoints contain many ordinary records in one bounded document.
// Four wire bytes of cumulative string-decoding work and four of structural
// headroom per encoded byte are an admission policy, not an NBT invariant.
fn document_limits(wire_bytes: usize) -> Result<NbtLimits, NbtError> {
    if wire_bytes > MAX_NBT_TOTAL_BYTES {
        return Err(NbtError::BudgetExceeded {
            resource: "wire-byte",
            limit: MAX_NBT_TOTAL_BYTES,
        });
    }
    let allocation_bytes = wire_bytes
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(MAX_NBT_ALLOCATION_BYTES))
        .ok_or(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            limit: MAX_NBT_ALLOCATION_BYTES,
        })?;
    Ok(NbtLimits {
        string_bytes: MAX_NBT_TOTAL_BYTES,
        allocation_bytes,
        ..PRODUCTION_LIMITS
    })
}

#[derive(Debug)]
struct NbtBudget {
    limits: NbtLimits,
    wire_bytes: usize,
    nodes: usize,
    compound_entries: usize,
    string_bytes: usize,
    allocation_bytes: usize,
    model_decoder_allocation: bool,
}

impl NbtBudget {
    fn production() -> Self {
        Self::new(PRODUCTION_LIMITS)
    }

    fn new(limits: NbtLimits) -> Self {
        Self {
            limits,
            wire_bytes: 0,
            nodes: 0,
            compound_entries: 0,
            string_bytes: 0,
            allocation_bytes: 0,
            model_decoder_allocation: false,
        }
    }

    fn charge_wire(&mut self, amount: usize) -> Result<(), NbtError> {
        charge_budget(
            &mut self.wire_bytes,
            amount,
            self.limits.wire_bytes,
            "wire-byte",
        )
    }

    fn charge_node(&mut self) -> Result<(), NbtError> {
        charge_budget(&mut self.nodes, 1, self.limits.nodes, "node")
    }

    fn charge_compound_entry(&mut self) -> Result<(), NbtError> {
        charge_budget(
            &mut self.compound_entries,
            1,
            self.limits.compound_entries,
            "compound-entry",
        )
    }

    fn charge_string(&mut self, amount: usize) -> Result<(), NbtError> {
        charge_budget(
            &mut self.string_bytes,
            amount,
            self.limits.string_bytes,
            "string-byte",
        )
    }

    fn charge_allocation(&mut self, amount: usize) -> Result<(), NbtError> {
        charge_budget(
            &mut self.allocation_bytes,
            amount,
            self.limits.allocation_bytes,
            "allocation-byte",
        )
    }

    fn charge_decoder_allocation(&mut self, amount: usize) -> Result<(), NbtError> {
        if self.model_decoder_allocation {
            self.charge_allocation(amount)?;
        }
        Ok(())
    }
}

fn charge_budget(
    used: &mut usize,
    amount: usize,
    limit: usize,
    resource: &'static str,
) -> Result<(), NbtError> {
    let next = used
        .checked_add(amount)
        .ok_or(NbtError::BudgetExceeded { resource, limit })?;
    if next > limit {
        return Err(NbtError::BudgetExceeded { resource, limit });
    }
    *used = next;
    Ok(())
}

fn capacity_bytes<T>(capacity: usize, limit: usize) -> Result<usize, NbtError> {
    capacity
        .checked_mul(size_of::<T>())
        .ok_or(NbtError::BudgetExceeded {
            resource: "allocation-byte",
            limit,
        })
}

fn budgeted_vec_with_capacity<T>(
    capacity: usize,
    budget: &mut NbtBudget,
    resource: &'static str,
) -> Result<Vec<T>, NbtError> {
    if capacity == 0 {
        return Ok(Vec::new());
    }
    let requested_bytes = capacity_bytes::<T>(capacity, budget.limits.allocation_bytes)?;
    budget.charge_allocation(requested_bytes)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| NbtError::AllocationFailed {
            resource,
            bytes: requested_bytes,
        })?;
    if values.capacity() > capacity {
        budget.charge_allocation(capacity_bytes::<T>(
            values.capacity() - capacity,
            budget.limits.allocation_bytes,
        )?)?;
    }
    Ok(values)
}

fn budgeted_string_with_capacity(
    capacity: usize,
    budget: &mut NbtBudget,
    resource: &'static str,
) -> Result<String, NbtError> {
    if capacity == 0 {
        return Ok(String::new());
    }
    budget.charge_allocation(capacity)?;
    let mut value = String::new();
    value
        .try_reserve_exact(capacity)
        .map_err(|_| NbtError::AllocationFailed {
            resource,
            bytes: capacity,
        })?;
    if value.capacity() > capacity {
        budget.charge_allocation(value.capacity() - capacity)?;
    }
    Ok(value)
}

fn compound_capacity(len: usize) -> usize {
    if len == 0 {
        0
    } else if len <= COMPOUND_RESERVE_CHUNK {
        len.next_power_of_two().max(4)
    } else {
        len.div_ceil(COMPOUND_RESERVE_CHUNK) * COMPOUND_RESERVE_CHUNK
    }
}

fn ensure_budgeted_compound_capacity(
    entries: &mut Vec<(String, Tag)>,
    budget: &mut NbtBudget,
) -> Result<(), NbtError> {
    if entries.len() < entries.capacity() {
        return Ok(());
    }
    let remaining = MAX_NBT_COMPOUND_ENTRIES.saturating_sub(entries.capacity());
    let additional = compound_capacity(entries.len() + 1)
        .saturating_sub(entries.capacity())
        .min(remaining)
        .max(1);
    let requested_bytes =
        capacity_bytes::<(String, Tag)>(additional, budget.limits.allocation_bytes)?;
    budget.charge_allocation(requested_bytes)?;
    let old_capacity = entries.capacity();
    entries
        .try_reserve_exact(additional)
        .map_err(|_| NbtError::AllocationFailed {
            resource: "compound entries",
            bytes: requested_bytes,
        })?;
    let requested_capacity = old_capacity.saturating_add(additional);
    if entries.capacity() > requested_capacity {
        budget.charge_allocation(capacity_bytes::<(String, Tag)>(
            entries.capacity() - requested_capacity,
            budget.limits.allocation_bytes,
        )?)?;
    }
    Ok(())
}

fn push_bounded<T>(stack: &mut Vec<T>, value: T) -> Result<(), NbtError> {
    if stack.len() == stack.capacity() {
        return Err(NbtError::BudgetExceeded {
            resource: "frame",
            limit: stack.capacity(),
        });
    }
    stack.push(value);
    Ok(())
}

// -----------------------------------------------------------------------
// Reader / writer
// -----------------------------------------------------------------------

fn ensure_remaining<B: Buf + ?Sized>(buf: &B, needed: usize) -> Result<(), NbtError> {
    let available = buf.remaining();
    if available < needed {
        return Err(NbtError::Underflow {
            needed: needed - available,
            available,
        });
    }
    Ok(())
}

fn consume<B: Buf + ?Sized>(buf: &B, budget: &mut NbtBudget, bytes: usize) -> Result<(), NbtError> {
    ensure_remaining(buf, bytes)?;
    budget.charge_wire(bytes)
}

fn read_string_with_budget<B: Buf>(
    buf: &mut B,
    budget: &mut NbtBudget,
) -> Result<String, NbtError> {
    consume(buf, budget, 2)?;
    let len = buf.get_u16() as usize;
    consume(buf, budget, len)?;
    budget.charge_string(len)?;

    let mut bytes = budgeted_vec_with_capacity(len, budget, "string input")?;
    bytes.resize(len, 0);
    buf.copy_to_slice(&mut bytes);

    let mut code_units = budgeted_vec_with_capacity(len, budget, "string code units")?;
    let mut offset = 0;
    while offset < bytes.len() {
        let first = bytes[offset];
        match first {
            0x01..=0x7F => {
                code_units.push(u16::from(first));
                offset += 1;
            }
            0xC0..=0xDF => {
                let second = *bytes.get(offset + 1).ok_or(NbtError::InvalidString)?;
                if second & 0xC0 != 0x80 {
                    return Err(NbtError::InvalidString);
                }

                let code_unit = (u16::from(first & 0x1F) << 6) | u16::from(second & 0x3F);
                if code_unit == 0 {
                    if first != 0xC0 || second != 0x80 {
                        return Err(NbtError::InvalidString);
                    }
                } else if code_unit < 0x80 {
                    return Err(NbtError::InvalidString);
                }

                code_units.push(code_unit);
                offset += 2;
            }
            0xE0..=0xEF => {
                let second = *bytes.get(offset + 1).ok_or(NbtError::InvalidString)?;
                let third = *bytes.get(offset + 2).ok_or(NbtError::InvalidString)?;
                if second & 0xC0 != 0x80 || third & 0xC0 != 0x80 {
                    return Err(NbtError::InvalidString);
                }

                let code_unit = (u16::from(first & 0x0F) << 12)
                    | (u16::from(second & 0x3F) << 6)
                    | u16::from(third & 0x3F);
                if code_unit < 0x800 {
                    return Err(NbtError::InvalidString);
                }

                code_units.push(code_unit);
                offset += 3;
            }
            _ => return Err(NbtError::InvalidString),
        }
    }

    let mut decoded = budgeted_string_with_capacity(len, budget, "decoded string")?;
    for scalar in char::decode_utf16(code_units) {
        decoded.push(scalar.map_err(|_| NbtError::InvalidString)?);
    }
    Ok(decoded)
}

fn modified_utf8_len(s: &str) -> Result<usize, NbtError> {
    s.encode_utf16().try_fold(0usize, |total, code_unit| {
        let encoded = match code_unit {
            0x0001..=0x007F => 1usize,
            0x0000..=0x07FF => 2,
            _ => 3,
        };
        total
            .checked_add(encoded)
            .ok_or(NbtError::StringTooLong(usize::MAX))
    })
}

fn write_string<B: BufMut>(buf: &mut B, s: &str) -> Result<(), NbtError> {
    let encoded_len = modified_utf8_len(s)?;
    let len = u16::try_from(encoded_len).map_err(|_| NbtError::StringTooLong(encoded_len))?;
    buf.put_u16(len);

    for code_unit in s.encode_utf16() {
        match code_unit {
            0x0001..=0x007F => buf.put_u8(code_unit as u8),
            0x0000..=0x07FF => {
                buf.put_u8(0xC0 | ((code_unit >> 6) as u8));
                buf.put_u8(0x80 | ((code_unit & 0x3F) as u8));
            }
            _ => {
                buf.put_u8(0xE0 | ((code_unit >> 12) as u8));
                buf.put_u8(0x80 | (((code_unit >> 6) & 0x3F) as u8));
                buf.put_u8(0x80 | ((code_unit & 0x3F) as u8));
            }
        }
    }

    Ok(())
}

fn collection_payload_bytes(len: usize, element_size: usize) -> Result<usize, NbtError> {
    let bytes = len
        .checked_mul(element_size)
        .ok_or(NbtError::ArrayTooLong(i64::MAX))?;
    if bytes > MAX_NBT_LENGTH {
        return Err(NbtError::ArrayTooLong(
            i64::try_from(bytes).unwrap_or(i64::MAX),
        ));
    }
    Ok(bytes)
}

fn read_length<B: Buf>(
    buf: &mut B,
    budget: &mut NbtBudget,
    element_size: usize,
) -> Result<usize, NbtError> {
    consume(buf, budget, 4)?;
    let len = buf.get_i32();
    if len < 0 {
        return Err(NbtError::NegativeLength(len));
    }
    let len = len as usize;
    collection_payload_bytes(len, element_size)?;
    Ok(len)
}

fn write_length<B: BufMut>(buf: &mut B, len: usize, element_size: usize) -> Result<(), NbtError> {
    collection_payload_bytes(len, element_size)?;
    let len = i32::try_from(len).map_err(|_| NbtError::ArrayTooLong(i64::MAX))?;
    buf.put_i32(len);
    Ok(())
}

fn ensure_container_depth(depth: usize) -> Result<(), NbtError> {
    if depth > MAX_NBT_DEPTH {
        return Err(NbtError::NestingTooDeep);
    }
    Ok(())
}

fn read_scalar_payload<B: Buf>(
    buf: &mut B,
    tag_id: u8,
    budget: &mut NbtBudget,
) -> Result<Tag, NbtError> {
    if !(tag_type::BYTE..=tag_type::LONG_ARRAY).contains(&tag_id) {
        return Err(NbtError::UnknownTag(tag_id));
    }
    budget.charge_node()?;
    match tag_id {
        tag_type::BYTE => {
            consume(buf, budget, 1)?;
            Ok(Tag::Byte(buf.get_i8()))
        }
        tag_type::SHORT => {
            consume(buf, budget, 2)?;
            Ok(Tag::Short(buf.get_i16()))
        }
        tag_type::INT => {
            consume(buf, budget, 4)?;
            Ok(Tag::Int(buf.get_i32()))
        }
        tag_type::LONG => {
            consume(buf, budget, 8)?;
            Ok(Tag::Long(buf.get_i64()))
        }
        tag_type::FLOAT => {
            consume(buf, budget, 4)?;
            Ok(Tag::Float(buf.get_f32()))
        }
        tag_type::DOUBLE => {
            consume(buf, budget, 8)?;
            Ok(Tag::Double(buf.get_f64()))
        }
        tag_type::BYTE_ARRAY => {
            let len = read_length(buf, budget, size_of::<i8>())?;
            consume(buf, budget, len)?;
            let mut data = budgeted_vec_with_capacity(len, budget, "byte array")?;
            data.resize(len, 0);
            // copy_to_slice wants u8; reinterpret via from_raw_parts is
            // unsafe and forbidden here, so do the cast element-by-element.
            for slot in &mut data {
                *slot = buf.get_i8();
            }
            Ok(Tag::ByteArray(data))
        }
        tag_type::STRING => Ok(Tag::String(read_string_with_budget(buf, budget)?)),
        tag_type::LIST => unreachable!("containers use the explicit-stack reader"),
        tag_type::COMPOUND => unreachable!("containers use the explicit-stack reader"),
        tag_type::INT_ARRAY => {
            let len = read_length(buf, budget, size_of::<i32>())?;
            let bytes = collection_payload_bytes(len, size_of::<i32>())?;
            consume(buf, budget, bytes)?;
            let mut data = budgeted_vec_with_capacity(len, budget, "int array")?;
            for _ in 0..len {
                data.push(buf.get_i32());
            }
            Ok(Tag::IntArray(data))
        }
        tag_type::LONG_ARRAY => {
            let len = read_length(buf, budget, size_of::<i64>())?;
            let bytes = collection_payload_bytes(len, size_of::<i64>())?;
            consume(buf, budget, bytes)?;
            let mut data = budgeted_vec_with_capacity(len, budget, "long array")?;
            for _ in 0..len {
                data.push(buf.get_i64());
            }
            Ok(Tag::LongArray(data))
        }
        _ => unreachable!("tag id range was checked above"),
    }
}

fn minimum_payload_bytes(tag_id: u8) -> Option<usize> {
    match tag_id {
        tag_type::BYTE => Some(1),
        tag_type::SHORT => Some(2),
        tag_type::INT | tag_type::FLOAT => Some(4),
        tag_type::LONG | tag_type::DOUBLE => Some(8),
        tag_type::BYTE_ARRAY | tag_type::INT_ARRAY | tag_type::LONG_ARRAY => Some(4),
        tag_type::STRING => Some(2),
        tag_type::LIST => Some(5),
        tag_type::COMPOUND => Some(1),
        _ => None,
    }
}

fn ensure_list_feasible<B: Buf>(buf: &B, len: usize, element_type: u8) -> Result<(), NbtError> {
    if len == 0 {
        return Ok(());
    }
    let minimum = minimum_payload_bytes(element_type).ok_or(NbtError::UnknownTag(element_type))?;
    let available = buf.remaining();
    if len > available / minimum {
        let required = len.saturating_mul(minimum);
        return Err(NbtError::Underflow {
            needed: required.saturating_sub(available),
            available,
        });
    }
    Ok(())
}

enum ReadFrame {
    Value {
        tag_id: u8,
        depth: usize,
    },
    List {
        element_type: u8,
        remaining: usize,
        elements: Vec<Tag>,
        depth: usize,
    },
    ListValue {
        element_type: u8,
        remaining: usize,
        elements: Vec<Tag>,
        depth: usize,
    },
    Compound {
        entries: Vec<(String, Tag)>,
        depth: usize,
    },
    CompoundValue {
        entries: Vec<(String, Tag)>,
        name: String,
        depth: usize,
    },
}

fn read_tag<B: Buf>(
    buf: &mut B,
    tag_id: u8,
    depth: usize,
    budget: &mut NbtBudget,
) -> Result<Tag, NbtError> {
    let mut stack = budgeted_vec_with_capacity(FRAME_STACK_CAPACITY, budget, "reader frames")?;
    push_bounded(&mut stack, ReadFrame::Value { tag_id, depth })?;
    let mut completed = None;

    while let Some(frame) = stack.pop() {
        match frame {
            ReadFrame::Value { tag_id, depth } => match tag_id {
                tag_type::LIST => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    consume(buf, budget, 1)?;
                    let element_type = buf.get_u8();
                    let len = read_length(buf, budget, size_of::<Tag>())?;
                    if element_type == tag_type::END && len > 0 {
                        return Err(NbtError::HeterogeneousList {
                            parent: tag_type::LIST,
                            found: tag_type::END,
                        });
                    }
                    ensure_list_feasible(buf, len, element_type)?;
                    let elements = budgeted_vec_with_capacity(len, budget, "list elements")?;
                    push_bounded(
                        &mut stack,
                        ReadFrame::List {
                            element_type,
                            remaining: len,
                            elements,
                            depth,
                        },
                    )?;
                }
                tag_type::COMPOUND => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    push_bounded(
                        &mut stack,
                        ReadFrame::Compound {
                            entries: Vec::new(),
                            depth,
                        },
                    )?;
                }
                _ => completed = Some(read_scalar_payload(buf, tag_id, budget)?),
            },
            ReadFrame::List {
                element_type,
                remaining,
                elements,
                depth,
            } => {
                if remaining == 0 {
                    completed = Some(Tag::List(ListTag {
                        element_type,
                        elements,
                    }));
                } else {
                    push_bounded(
                        &mut stack,
                        ReadFrame::ListValue {
                            element_type,
                            remaining,
                            elements,
                            depth,
                        },
                    )?;
                    push_bounded(
                        &mut stack,
                        ReadFrame::Value {
                            tag_id: element_type,
                            depth: depth + 1,
                        },
                    )?;
                }
            }
            ReadFrame::ListValue {
                element_type,
                remaining,
                mut elements,
                depth,
            } => {
                let value = completed
                    .take()
                    .expect("list child frame completes one tag");
                elements.push(value);
                push_bounded(
                    &mut stack,
                    ReadFrame::List {
                        element_type,
                        remaining: remaining - 1,
                        elements,
                        depth,
                    },
                )?;
            }
            ReadFrame::Compound { mut entries, depth } => {
                consume(buf, budget, 1)?;
                let child_type = buf.get_u8();
                if child_type == tag_type::END {
                    completed = Some(Tag::Compound(entries));
                    continue;
                }
                if entries.len() >= MAX_NBT_COMPOUND_ENTRIES {
                    return Err(NbtError::BudgetExceeded {
                        resource: "compound-entry-per-container",
                        limit: MAX_NBT_COMPOUND_ENTRIES,
                    });
                }
                budget.charge_compound_entry()?;
                ensure_budgeted_compound_capacity(&mut entries, budget)?;
                let name = read_string_with_budget(buf, budget)?;
                push_bounded(
                    &mut stack,
                    ReadFrame::CompoundValue {
                        entries,
                        name,
                        depth,
                    },
                )?;
                push_bounded(
                    &mut stack,
                    ReadFrame::Value {
                        tag_id: child_type,
                        depth: depth + 1,
                    },
                )?;
            }
            ReadFrame::CompoundValue {
                mut entries,
                name,
                depth,
            } => {
                let value = completed
                    .take()
                    .expect("compound child frame completes one tag");
                entries.push((name, value));
                push_bounded(&mut stack, ReadFrame::Compound { entries, depth })?;
            }
        }
    }

    completed.ok_or(NbtError::Underflow {
        needed: 1,
        available: 0,
    })
}

fn validate_string(s: &str, budget: &mut NbtBudget) -> Result<(), NbtError> {
    let encoded_len = modified_utf8_len(s)?;
    u16::try_from(encoded_len).map_err(|_| NbtError::StringTooLong(encoded_len))?;
    budget.charge_string(encoded_len)?;
    budget.charge_decoder_allocation(encoded_len * 4)?;
    budget.charge_wire(2usize.saturating_add(encoded_len))
}

fn validate_scalar_payload(tag: &Tag, budget: &mut NbtBudget) -> Result<(), NbtError> {
    budget.charge_node()?;
    match tag {
        Tag::Byte(_) => budget.charge_wire(1),
        Tag::Short(_) => budget.charge_wire(2),
        Tag::Int(_) | Tag::Float(_) => budget.charge_wire(4),
        Tag::Long(_) | Tag::Double(_) => budget.charge_wire(8),
        Tag::ByteArray(data) => {
            let bytes = collection_payload_bytes(data.len(), size_of::<i8>())?;
            budget.charge_decoder_allocation(bytes)?;
            budget.charge_wire(4usize.saturating_add(bytes))
        }
        Tag::String(s) => validate_string(s, budget),
        Tag::List(_) => unreachable!("containers use the explicit-stack validator"),
        Tag::Compound(_) => unreachable!("containers use the explicit-stack validator"),
        Tag::IntArray(data) => {
            let bytes = collection_payload_bytes(data.len(), size_of::<i32>())?;
            budget.charge_decoder_allocation(bytes)?;
            budget.charge_wire(4usize.saturating_add(bytes))
        }
        Tag::LongArray(data) => {
            let bytes = collection_payload_bytes(data.len(), size_of::<i64>())?;
            budget.charge_decoder_allocation(bytes)?;
            budget.charge_wire(4usize.saturating_add(bytes))
        }
    }
}

enum ValidationFrame<'a> {
    Tag {
        tag: &'a Tag,
        depth: usize,
    },
    List {
        list: &'a ListTag,
        index: usize,
        depth: usize,
    },
    Compound {
        entries: &'a [(String, Tag)],
        index: usize,
        depth: usize,
    },
}

fn validate_payload(tag: &Tag, depth: usize, budget: &mut NbtBudget) -> Result<(), NbtError> {
    let mut stack = vec![ValidationFrame::Tag { tag, depth }];
    while let Some(frame) = stack.pop() {
        match frame {
            ValidationFrame::Tag { tag, depth } => match tag {
                Tag::List(list) => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    let bytes = collection_payload_bytes(list.elements.len(), size_of::<Tag>())?;
                    budget.charge_decoder_allocation(bytes)?;
                    if list.element_type == tag_type::END && !list.elements.is_empty() {
                        return Err(NbtError::HeterogeneousList {
                            parent: tag_type::LIST,
                            found: tag_type::END,
                        });
                    }
                    budget.charge_wire(5)?;
                    stack.push(ValidationFrame::List {
                        list,
                        index: 0,
                        depth,
                    });
                }
                Tag::Compound(entries) => {
                    budget.charge_node()?;
                    ensure_container_depth(depth)?;
                    if entries.len() > MAX_NBT_COMPOUND_ENTRIES {
                        return Err(NbtError::BudgetExceeded {
                            resource: "compound-entry-per-container",
                            limit: MAX_NBT_COMPOUND_ENTRIES,
                        });
                    }
                    if budget.model_decoder_allocation {
                        let capacity = compound_capacity(entries.len());
                        let bytes = capacity_bytes::<(String, Tag)>(
                            capacity,
                            budget.limits.allocation_bytes,
                        )?;
                        budget.charge_allocation(bytes)?;
                    }
                    stack.push(ValidationFrame::Compound {
                        entries,
                        index: 0,
                        depth,
                    });
                }
                _ => validate_scalar_payload(tag, budget)?,
            },
            ValidationFrame::List { list, index, depth } => {
                let Some(element) = list.elements.get(index) else {
                    continue;
                };
                if element.type_id() != list.element_type {
                    return Err(NbtError::HeterogeneousList {
                        parent: list.element_type,
                        found: element.type_id(),
                    });
                }
                stack.push(ValidationFrame::List {
                    list,
                    index: index + 1,
                    depth,
                });
                stack.push(ValidationFrame::Tag {
                    tag: element,
                    depth: depth + 1,
                });
            }
            ValidationFrame::Compound {
                entries,
                index,
                depth,
            } => {
                let Some((name, value)) = entries.get(index) else {
                    budget.charge_wire(1)?;
                    continue;
                };
                budget.charge_compound_entry()?;
                budget.charge_wire(1)?;
                validate_string(name, budget)?;
                stack.push(ValidationFrame::Compound {
                    entries,
                    index: index + 1,
                    depth,
                });
                stack.push(ValidationFrame::Tag {
                    tag: value,
                    depth: depth + 1,
                });
            }
        }
    }
    Ok(())
}

fn write_scalar_payload<B: BufMut>(buf: &mut B, tag: &Tag) {
    match tag {
        Tag::Byte(v) => buf.put_i8(*v),
        Tag::Short(v) => buf.put_i16(*v),
        Tag::Int(v) => buf.put_i32(*v),
        Tag::Long(v) => buf.put_i64(*v),
        Tag::Float(v) => buf.put_f32(*v),
        Tag::Double(v) => buf.put_f64(*v),
        Tag::ByteArray(data) => {
            write_length(buf, data.len(), size_of::<i8>()).expect("NBT write preflight");
            buf.put_slice(bytemuck::cast_slice(data));
        }
        Tag::String(s) => write_string(buf, s).expect("NBT write preflight"),
        Tag::List(_) => unreachable!("containers use the explicit-stack writer"),
        Tag::Compound(_) => unreachable!("containers use the explicit-stack writer"),
        Tag::IntArray(data) => {
            write_length(buf, data.len(), size_of::<i32>()).expect("NBT write preflight");
            for v in data {
                buf.put_i32(*v);
            }
        }
        Tag::LongArray(data) => {
            write_length(buf, data.len(), size_of::<i64>()).expect("NBT write preflight");
            for v in data {
                buf.put_i64(*v);
            }
        }
    }
}

enum WriteFrame<'a> {
    Tag(&'a Tag),
    List {
        elements: &'a [Tag],
        index: usize,
    },
    Compound {
        entries: &'a [(String, Tag)],
        index: usize,
    },
}

fn write_payload_unchecked<B: BufMut>(buf: &mut B, tag: &Tag) {
    let mut stack = vec![WriteFrame::Tag(tag)];
    while let Some(frame) = stack.pop() {
        match frame {
            WriteFrame::Tag(tag) => match tag {
                Tag::List(list) => {
                    buf.put_u8(list.element_type);
                    write_length(buf, list.elements.len(), size_of::<Tag>())
                        .expect("NBT write preflight");
                    stack.push(WriteFrame::List {
                        elements: &list.elements,
                        index: 0,
                    });
                }
                Tag::Compound(entries) => stack.push(WriteFrame::Compound { entries, index: 0 }),
                _ => write_scalar_payload(buf, tag),
            },
            WriteFrame::List { elements, index } => {
                let Some(element) = elements.get(index) else {
                    continue;
                };
                stack.push(WriteFrame::List {
                    elements,
                    index: index + 1,
                });
                stack.push(WriteFrame::Tag(element));
            }
            WriteFrame::Compound { entries, index } => {
                let Some((name, value)) = entries.get(index) else {
                    buf.put_u8(tag_type::END);
                    continue;
                };
                buf.put_u8(value.type_id());
                write_string(buf, name).expect("NBT write preflight");
                stack.push(WriteFrame::Compound {
                    entries,
                    index: index + 1,
                });
                stack.push(WriteFrame::Tag(value));
            }
        }
    }
}

// -----------------------------------------------------------------------
// Public entry points
// -----------------------------------------------------------------------

fn read_network_with_budget<B: Buf>(buf: &mut B, budget: &mut NbtBudget) -> Result<Tag, NbtError> {
    consume(buf, budget, 1)?;
    let tag_id = buf.get_u8();
    if tag_id != tag_type::COMPOUND {
        return Err(NbtError::NotCompound(tag_id));
    }
    read_tag(buf, tag_id, 1, budget)
}

/// Read a *network-format* compound NBT root. For component codecs whose
/// anonymous root may also be a string or list, use [`read_network_value`].
pub fn read_network<B: Buf>(buf: &mut B) -> Result<Tag, NbtError> {
    let mut budget = NbtBudget::production();
    read_network_with_budget(buf, &mut budget)
}

/// Write a *network-format* NBT root. `root` must be a [`Tag::Compound`].
///
/// The complete tree is validated and budgeted before the first byte is written,
/// so any semantic error leaves the caller's buffer unchanged.
pub fn write_network<B: BufMut>(buf: &mut B, root: &Tag) -> Result<(), NbtError> {
    if !matches!(root, Tag::Compound(_)) {
        return Err(NbtError::RootMustBeCompound(root.type_id()));
    }
    let mut budget = NbtBudget::production();
    budget.charge_wire(1)?;
    validate_payload(root, 1, &mut budget)?;

    buf.put_u8(tag_type::COMPOUND);
    write_payload_unchecked(buf, root);
    Ok(())
}

/// Read an anonymous NBT value (vanilla `ByteBufCodecs.TAG`), including the
/// primitive and list roots used by text components. Production budgets,
/// depth limits and tag validation are identical to [`read_network`].
pub fn read_network_value<B: Buf>(buf: &mut B) -> Result<Tag, NbtError> {
    let mut budget = NbtBudget::production();
    consume(buf, &mut budget, 1)?;
    let tag_id = buf.get_u8();
    read_tag(buf, tag_id, 1, &mut budget)
}

/// Write an anonymous NBT value with the same atomic preflight and budgets as
/// [`write_network`], without imposing the compound-only root contract.
pub fn write_network_value<B: BufMut>(buf: &mut B, root: &Tag) -> Result<(), NbtError> {
    let mut budget = NbtBudget::production();
    budget.charge_wire(1)?;
    validate_payload(root, 1, &mut budget)?;
    buf.put_u8(root.type_id());
    write_payload_unchecked(buf, root);
    Ok(())
}

/// Read a *named* NBT root: the disk format used by region files and
/// `.nbt` files. Returns the root tag's name alongside its value.
pub fn read_named<B: Buf>(buf: &mut B) -> Result<(String, Tag), NbtError> {
    read_named_with_budget(buf, &mut NbtBudget::production())
}

/// Read one bounded entity-checkpoint document, without changing the generic
/// NBT policy. The caller must reject trailing bytes after the named root.
/// Allocation admission is proportional to the bounded input's actual length;
/// the wire, collection, depth, node, entry and u16-string ceilings still apply.
pub fn read_named_document<B: Buf>(buf: &mut B) -> Result<(String, Tag), NbtError> {
    let limits = document_limits(buf.remaining())?;
    read_named_with_budget(buf, &mut NbtBudget::new(limits))
}

fn read_named_with_budget<B: Buf>(
    buf: &mut B,
    budget: &mut NbtBudget,
) -> Result<(String, Tag), NbtError> {
    consume(buf, budget, 1)?;
    let tag_id = buf.get_u8();
    if tag_id != tag_type::COMPOUND {
        return Err(NbtError::NotCompound(tag_id));
    }
    let name = read_string_with_budget(buf, budget)?;
    let value = read_tag(buf, tag_id, 1, budget)?;
    Ok((name, value))
}

/// Write a *named* NBT root. Validation is atomic with respect to the caller's
/// buffer: semantic failures are reported before the first byte is emitted.
pub fn write_named<B: BufMut>(buf: &mut B, name: &str, root: &Tag) -> Result<(), NbtError> {
    write_named_with_budget(buf, name, root, &mut NbtBudget::production())
}

/// Write a bounded entity-checkpoint document. Preflight models the reader's
/// cumulative allocations and checks the actual encoded-length allowance before
/// emitting any bytes, so a successful document is admissible on restart.
pub fn write_named_document<B: BufMut>(
    buf: &mut B,
    name: &str,
    root: &Tag,
) -> Result<(), NbtError> {
    let mut budget = NbtBudget::new(document_limits(MAX_NBT_TOTAL_BYTES)?);
    budget.model_decoder_allocation = true;
    write_named_with_budget(buf, name, root, &mut budget)
}

fn write_named_with_budget<B: BufMut>(
    buf: &mut B,
    name: &str,
    root: &Tag,
    budget: &mut NbtBudget,
) -> Result<(), NbtError> {
    if !matches!(root, Tag::Compound(_)) {
        return Err(NbtError::RootMustBeCompound(root.type_id()));
    }
    budget.charge_wire(1)?;
    if budget.model_decoder_allocation {
        let bytes =
            capacity_bytes::<ReadFrame>(FRAME_STACK_CAPACITY, budget.limits.allocation_bytes)?;
        budget.charge_allocation(bytes)?;
    }
    validate_string(name, budget)?;
    validate_payload(root, 1, budget)?;
    if budget.model_decoder_allocation {
        budget.limits = document_limits(budget.wire_bytes)?;
        budget.charge_allocation(0)?;
    }

    buf.put_u8(tag_type::COMPOUND);
    write_string(buf, name).expect("NBT write preflight");
    write_payload_unchecked(buf, root);
    Ok(())
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
#[path = "nbt_tests.rs"]
mod tests;
