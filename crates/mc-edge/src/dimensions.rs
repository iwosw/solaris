//! Vanilla registry indices, never hard-coded wire dimension IDs.
use mc_core::dimensions::Dimension;
use mc_data::VanillaData;

pub(crate) fn registry_ids(data: &VanillaData) -> anyhow::Result<[i32; 3]> {
    let registry = data
        .registry("dimension_type")
        .ok_or_else(|| anyhow::anyhow!("dimension_type registry missing"))?;
    let mut ids = [0; 3];
    for dimension in Dimension::ALL {
        let index = registry
            .entries
            .iter()
            .position(|entry| entry.as_str() == dimension.name())
            .ok_or_else(|| {
                anyhow::anyhow!("dimension_type registry missing {}", dimension.name())
            })?;
        ids[dimension as usize] = i32::try_from(index)?;
    }
    Ok(ids)
}

#[cfg(test)]
#[path = "dimensions_tests.rs"]
mod tests;
