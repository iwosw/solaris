//! ConduitBlockEntity 26.1.2 custom NBT: only `Target` (UUIDUtil.CODEC).
use mc_nbt::Tag;
use uuid::Uuid;

/// Read the optional target, rejecting malformed or duplicate durable fields.
pub fn target(fields: &[(String, Tag)]) -> Result<Option<Uuid>, String> {
    let mut found = None;
    for (name, value) in fields {
        if name != "Target" {
            continue;
        }
        if found.is_some() {
            return Err("conduit Target: duplicate field".into());
        }
        let Tag::IntArray(words) = value else {
            return Err("conduit Target: expected TAG_Int_Array[4]".into());
        };
        let words: &[i32; 4] = words
            .as_slice()
            .try_into()
            .map_err(|_| "conduit Target: expected TAG_Int_Array[4]".to_owned())?;
        found =
            Some(Uuid::from_u128(words.iter().fold(0_u128, |uuid, word| {
                (uuid << 32) | u128::from(*word as u32)
            })));
    }
    Ok(found)
}

pub fn fields(target: Option<Uuid>) -> Vec<(String, Tag)> {
    target.map_or_else(Vec::new, |uuid| {
        let value = uuid.as_u128();
        vec![(
            "Target".into(),
            Tag::IntArray(
                (0..4)
                    .rev()
                    .map(|index| (value >> (index * 32)) as i32)
                    .collect(),
            ),
        )]
    })
}

/// Validate a network block-entity image if it belongs to a conduit.
pub fn validate(bytes: &[u8]) -> Result<(), String> {
    let mut reader = bytes;
    let tag =
        mc_nbt::read_network(&mut reader).map_err(|err| format!("block entity NBT: {err}"))?;
    let Tag::Compound(fields) = tag else {
        return Err("block entity NBT: expected TAG_Compound".into());
    };
    if fields
        .iter()
        .any(|(key, value)| key == "id" && value == &Tag::String("minecraft:conduit".into()))
    {
        if !reader.is_empty() {
            return Err("conduit NBT: trailing bytes".into());
        }
        target(&fields)?;
    }
    Ok(())
}
