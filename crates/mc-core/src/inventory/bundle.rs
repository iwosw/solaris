//! Vanilla `BundleItem` / `BundleContents` over the `minecraft:bundle_contents` component.
//!
//! The contents live in the stack's preserved component as native NBT (a list of item
//! compounds, newest first), so they persist, merge and reach clients through the existing
//! component codecs. Weight is exact rational arithmetic: a stack weighs `1 / max_stack`,
//! a nested bundle `1/16` plus its contents, and the total never exceeds one.

use super::ItemRules;
use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::{can_stack, canonicalize_empty};
use mc_data::item_stack::PreservedItemComponent;
use mc_nbt::{ListTag, Tag, tag_type};

const COMPONENT: &str = "minecraft:bundle_contents";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Weight {
    n: i128,
    d: i128,
}

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

impl Weight {
    const ZERO: Self = Self { n: 0, d: 1 };

    fn new(n: i128, d: i128) -> Self {
        let g = gcd(n, d).max(1);
        Self { n: n / g, d: d / g }
    }

    fn add(self, other: Self) -> Option<Self> {
        let d = self.d.checked_mul(other.d)?;
        let n = self
            .n
            .checked_mul(other.d)?
            .checked_add(other.n.checked_mul(self.d)?)?;
        Some(Self::new(n, d))
    }

    fn times(self, count: i128) -> Option<Self> {
        Some(Self::new(self.n.checked_mul(count)?, self.d))
    }

    /// `floor(self / unit)` for a positive `unit`.
    fn whole_units_of(self, unit: Self) -> Option<i128> {
        Some(self.n.checked_mul(unit.d)? / self.d.checked_mul(unit.n)?)
    }
}

fn name<'a>(rules: &'a ItemRules<'_>, stack: &ItemStack) -> Option<&'a str> {
    rules.items.name_of(stack.item_id).map(|id| id.as_str())
}

pub(super) fn is_bundle(rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
    !stack.is_empty()
        && name(rules, stack)
            .is_some_and(|name| name == "minecraft:bundle" || name.ends_with("_bundle"))
}

/// Vanilla `Item.canFitInsideContainerItems`: everything except shulker boxes.
fn can_fit(rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
    !stack.is_empty() && !name(rules, stack).is_some_and(|name| name.ends_with("shulker_box"))
}

/// The bundle's stacks, newest first; `None` when the stored component is unreadable.
pub(super) fn read(rules: &ItemRules<'_>, bundle: &ItemStack) -> Option<Vec<ItemStack>> {
    let Some(component) = bundle
        .extended()
        .preserved_components
        .iter()
        .find(|component| component.name == COMPONENT)
    else {
        return Some(Vec::new());
    };
    let mut bytes = component.value.as_slice();
    let Tag::List(list) = mc_nbt::read_network_value(&mut bytes).ok()? else {
        return None;
    };
    if !bytes.is_empty() {
        return None;
    }
    list.elements
        .iter()
        .map(|tag| {
            let Tag::Compound(fields) = tag else {
                return None;
            };
            mc_data::item_nbt::stack_from_fields(rules.items, fields)
                .ok()
                .flatten()
        })
        .collect()
}

/// Store `contents`; an empty bundle carries no component, like a fresh one.
pub(super) fn write(rules: &ItemRules<'_>, bundle: &mut ItemStack, contents: &[ItemStack]) -> bool {
    let value = if contents.is_empty() {
        None
    } else {
        let Some(elements) = contents
            .iter()
            .map(|stack| {
                mc_data::item_nbt::stack_to_fields(rules.items, stack)
                    .ok()
                    .map(Tag::Compound)
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        let mut bytes = Vec::new();
        let list = Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements,
        });
        if mc_nbt::write_network_value(&mut bytes, &list).is_err() {
            return false;
        }
        Some(bytes)
    };
    let components = &mut bundle.extended_mut().preserved_components;
    components.retain(|component| component.name.trim_start_matches('!') != COMPONENT);
    if let Some(value) = value {
        components.push(PreservedItemComponent {
            name: COMPONENT.into(),
            value,
            baseline: None,
        });
    }
    true
}

fn weight(rules: &ItemRules<'_>, stack: &ItemStack) -> Option<Weight> {
    if is_bundle(rules, stack) {
        let inner = total(rules, &read(rules, stack)?)?;
        return Weight::new(1, 16).add(inner);
    }
    Some(Weight::new(1, i128::from(rules.max_stack(stack).max(1))))
}

fn total(rules: &ItemRules<'_>, contents: &[ItemStack]) -> Option<Weight> {
    contents.iter().try_fold(Weight::ZERO, |sum, stack| {
        sum.add(weight(rules, stack)?.times(i128::from(stack.count))?)
    })
}

/// How many of `stack` still fit (vanilla `BundleContents.Mutable.getMaxAmountToAdd`).
pub(super) fn room(rules: &ItemRules<'_>, contents: &[ItemStack], stack: &ItemStack) -> i32 {
    if !can_fit(rules, stack) {
        return 0;
    }
    let (Some(used), Some(each)) = (total(rules, contents), weight(rules, stack)) else {
        return 0;
    };
    let free = Weight::new(used.d - used.n, used.d);
    free.whole_units_of(each)
        .map_or(0, |units| units.clamp(0, i128::from(i32::MAX)) as i32)
}

/// Vanilla `Mutable.tryInsert`: moves what fits out of `stack`, merging into an existing
/// stackable entry, and returns the moved count.
pub(super) fn insert(
    rules: &ItemRules<'_>,
    contents: &mut Vec<ItemStack>,
    stack: &mut ItemStack,
) -> i32 {
    let moved = room(rules, contents, stack).min(stack.count);
    if moved <= 0 {
        return 0;
    }
    let mut entry = stack.clone();
    entry.count = moved;
    if rules.max_stack(stack) > 1
        && let Some(index) = contents.iter().position(|old| can_stack(old, stack))
    {
        entry.count += contents.remove(index).count;
    }
    contents.insert(0, entry);
    stack.count -= moved;
    canonicalize_empty(stack);
    moved
}

/// Vanilla `Mutable.removeOne`: the newest stack leaves the bundle whole.
pub(super) fn remove_first(contents: &mut Vec<ItemStack>) -> Option<ItemStack> {
    (!contents.is_empty()).then(|| contents.remove(0))
}
