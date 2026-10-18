//! Native scheduled work uses absolute u64 deadlines, not Anvil i32 delays.
use super::{NativeError, codec};
use crate::{BlockPos, BlockRegistry, Chunk, ScheduledBlockTick, ScheduledFluidTick};
use mc_data::Identifier;
use mc_nbt::{ListTag, Tag, tag_type};

const BLOCK: &str = "SolarisNativeBlockTicksV1";
const FLUID: &str = "SolarisNativeFluidTicksV1";

fn tick(pos: BlockPos, name: &Identifier, deadline: u64, priority: i32, sequence: u64) -> Tag {
    Tag::Compound(vec![
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("type".into(), Tag::String(name.to_string())),
        // Long carries the exact 64 bits, including values above i64::MAX.
        ("deadline".into(), Tag::Long(deadline as i64)),
        ("priority".into(), Tag::Int(priority)),
        ("sequence".into(), Tag::Long(sequence as i64)),
    ])
}

pub(super) fn append(root: &mut Tag, chunk: &Chunk) -> Result<(), NativeError> {
    let fields = codec::compound_mut(root)?;
    fields.retain(|(key, _)| key != BLOCK && key != FLUID);
    let blocks = chunk
        .scheduled_block_ticks()
        .iter()
        .map(|value| {
            tick(
                value.pos,
                &value.block,
                value.trigger_tick,
                value.priority,
                value.sequence(),
            )
        })
        .collect();
    let fluids = chunk
        .scheduled_fluid_ticks()
        .iter()
        .map(|value| {
            tick(
                value.pos,
                &value.fluid,
                value.trigger_tick,
                value.priority,
                value.sequence(),
            )
        })
        .collect();
    fields.push((
        BLOCK.into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: blocks,
        }),
    ));
    fields.push((
        FLUID.into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: fluids,
        }),
    ));
    Ok(())
}

fn decode<T>(
    root: &Tag,
    key: &str,
    chunk: &Chunk,
    blocks: &BlockRegistry,
    make: impl Fn(BlockPos, Identifier, u64, i32, u64) -> T,
) -> Result<Vec<T>, NativeError> {
    let Tag::Compound(root) = root else {
        return Err(NativeError::Invalid("native tick root must be compound"));
    };
    let mut matching = root.iter().filter(|(name, _)| name == key);
    let Some((_, Tag::List(list))) = matching.next() else {
        return Err(NativeError::Invalid("native ticks missing"));
    };
    if matching.next().is_some() {
        return Err(NativeError::Invalid("native ticks duplicated"));
    }
    if list.element_type != tag_type::COMPOUND {
        return Err(NativeError::Invalid("native ticks must be compounds"));
    }
    let mut seen = std::collections::BTreeSet::new();
    list.elements
        .iter()
        .map(|tag| {
            let Tag::Compound(fields) = tag else {
                return Err(NativeError::Invalid("invalid native tick"));
            };
            if fields.len() != 7 {
                return Err(NativeError::Invalid("invalid native tick fields"));
            }
            let field = |name: &str| -> Result<&Tag, NativeError> {
                let mut values = fields.iter().filter(|(key, _)| key == name);
                let (_, value) = values
                    .next()
                    .ok_or(NativeError::Invalid("native tick field missing"))?;
                if values.next().is_some() {
                    return Err(NativeError::Invalid("native tick field duplicated"));
                }
                Ok(value)
            };
            let int = |name| match field(name)? {
                Tag::Int(value) => Ok(*value),
                _ => Err(NativeError::Invalid("invalid native tick integer")),
            };
            let long = |name| match field(name)? {
                Tag::Long(value) => Ok(*value as u64),
                _ => Err(NativeError::Invalid("invalid native tick long")),
            };
            let pos = BlockPos {
                x: int("x")?,
                y: int("y")?,
                z: int("z")?,
            };
            if pos.x.div_euclid(16) != chunk.pos.x
                || pos.z.div_euclid(16) != chunk.pos.z
                || !(chunk.geometry().min_y()..chunk.geometry().max_y()).contains(&pos.y)
            {
                return Err(NativeError::Invalid("native tick outside owning chunk"));
            }
            let Tag::String(name) = field("type")? else {
                return Err(NativeError::Invalid("native tick type missing"));
            };
            let kind = if key == FLUID { "fluid" } else { "block" };
            let name = Identifier::parse(name).map_err(|_| {
                NativeError::Source(format!("invalid native {kind} tick type {name:?}"))
            })?;
            let supported = if key == FLUID {
                ScheduledFluidTick::fluid_kind(&name).is_some()
            } else {
                blocks.block(&name).is_some()
            };
            if !supported {
                return Err(NativeError::Source(format!(
                    "unknown native {kind} tick type {name}"
                )));
            }
            let deadline = long("deadline")?;
            let priority = int("priority")?;
            let sequence = long("sequence")?;
            if !seen.insert(sequence) {
                return Err(NativeError::Invalid("duplicate native tick sequence"));
            }
            Ok(make(pos, name, deadline, priority, sequence))
        })
        .collect()
}

pub(super) fn restore(
    root: &Tag,
    chunk: &mut Chunk,
    blocks: &BlockRegistry,
) -> Result<(), NativeError> {
    let block_ticks = decode(root, BLOCK, chunk, blocks, ScheduledBlockTick::from_storage)?;
    let fluid_ticks = decode(root, FLUID, chunk, blocks, ScheduledFluidTick::from_storage)?;
    chunk.load_scheduled_block_ticks(block_ticks);
    chunk.load_scheduled_fluid_ticks(fluid_ticks);
    chunk.extras.retain(|(key, _)| key != BLOCK && key != FLUID);
    Ok(())
}

#[cfg(test)]
#[path = "ticks_tests.rs"]
mod tests;
