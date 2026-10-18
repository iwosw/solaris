//! Native banner tag membership; menu order comes from the emitted raw-ID vectors.
use crate::{Identifier, Registry, tags::TagsData};

impl TagsData {
    /// Retain the banner registry in exactly the order sent by RegistryData.
    #[must_use]
    pub fn with_banner_pattern_registry(mut self, registry: &Registry) -> Self {
        self.banner_pattern_entries = registry.entries.clone();
        self
    }

    #[must_use]
    pub fn banner_pattern(&self, raw_id: i32) -> Option<&Identifier> {
        self.banner_pattern_entries
            .get(usize::try_from(raw_id).ok()?)
    }
}

pub(super) fn add(tags: &mut TagsData) {
    let entries = &crate::required_registry_entries()["banner_pattern"];
    tags.banner_pattern_entries = entries.clone();
    let registry = tags
        .registries
        .entry(Identifier::parse("minecraft:banner_pattern").expect("static registry id"))
        .or_default();
    // Expanded from the pinned native tags/banner_pattern JSON. This is tag
    // membership, not a second selectable-pattern catalogue: sorted raw IDs
    // match the full sidecar tag resolver and are the UpdateTags payload.
    for (tag, members) in [
        (
            "no_item_required",
            &[
                "square_bottom_left",
                "square_bottom_right",
                "square_top_left",
                "square_top_right",
                "stripe_bottom",
                "stripe_top",
                "stripe_left",
                "stripe_right",
                "stripe_center",
                "stripe_middle",
                "stripe_downright",
                "stripe_downleft",
                "small_stripes",
                "cross",
                "straight_cross",
                "triangle_bottom",
                "triangle_top",
                "triangles_bottom",
                "triangles_top",
                "diagonal_left",
                "diagonal_up_right",
                "diagonal_up_left",
                "diagonal_right",
                "circle",
                "rhombus",
                "half_vertical",
                "half_horizontal",
                "half_vertical_right",
                "half_horizontal_bottom",
                "border",
                "gradient",
                "gradient_up",
            ][..],
        ),
        ("pattern_item/flower", &["flower"][..]),
        ("pattern_item/flow", &["flow"][..]),
        ("pattern_item/mojang", &["mojang"][..]),
        ("pattern_item/creeper", &["creeper"][..]),
        ("pattern_item/globe", &["globe"][..]),
        ("pattern_item/bordure_indented", &["curly_border"][..]),
        ("pattern_item/skull", &["skull"][..]),
        ("pattern_item/piglin", &["piglin"][..]),
        ("pattern_item/guster", &["guster"][..]),
        ("pattern_item/field_masoned", &["bricks"][..]),
    ] {
        let mut ids: Vec<i32> = members
            .iter()
            .map(|name| {
                entries
                    .iter()
                    .position(|entry| entry.path() == *name)
                    .expect("native banner pattern is in the emitted registry")
                    as i32
            })
            .collect();
        ids.sort_unstable();
        registry.insert(
            Identifier::parse(format!("minecraft:{tag}")).expect("native banner tag"),
            ids,
        );
    }
}
