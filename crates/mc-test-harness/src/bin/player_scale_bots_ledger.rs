// A click only completes when received source debit and recipient credit balance.
// Natural furnace progress or a generic menu update cannot stand in for a click.
#[derive(Clone)]
struct MenuTransfer {
    menu: usize,
    slot: usize,
    item_id: u32,
    source_count: i64,
    other_count: i64,
    quick_move: bool,
}
impl MenuReceipt {
    fn image_slot(&self, slot: usize) -> Option<&SlotImage> {
        self.final_slots
            .as_ref()?
            .iter()
            .chain(self.final_player_inventory.as_ref()?.iter())
            .find(|image| image.slot == slot)
    }
    fn item_count_except(&self, item_id: u32, except: usize) -> i64 {
        self.final_slots
            .iter()
            .flatten()
            .chain(self.final_player_inventory.iter().flatten())
            .chain(self.carried.iter())
            .filter(|slot| slot.slot != except && slot.item_id == Some(item_id))
            .map(|slot| i64::from(slot.count))
            .sum()
    }
}
impl MenuTransfer {
    fn capture(menu: usize, image: &MenuReceipt, slot: usize, quick_move: bool) -> Option<Self> {
        let source = image.image_slot(slot)?;
        let item_id = source
            .item_id
            .or_else(|| image.carried.as_ref().and_then(|c| c.item_id))?;
        if !source.plain || image.carried.as_ref().is_some_and(|c| !c.plain) {
            return None;
        }
        Some(Self {
            menu,
            slot,
            item_id,
            source_count: if source.item_id == Some(item_id) {
                i64::from(source.count)
            } else {
                0
            },
            other_count: image.item_count_except(item_id, slot),
            quick_move,
        })
    }
    fn completed(&self, image: &MenuReceipt) -> bool {
        let Some(source) = image.image_slot(self.slot) else {
            return false;
        };
        let count = if source.item_id == Some(self.item_id) {
            i64::from(source.count)
        } else {
            0
        };
        let debit = self.source_count - count;
        let credit = image.item_count_except(self.item_id, self.slot) - self.other_count;
        debit != 0 && (!self.quick_move || debit > 0) && debit == credit
    }
}
fn plain_hashed_stack(image: Option<&SlotImage>) -> Option<HashedStack> {
    match image {
        None => Some(HashedStack::empty()),
        Some(image) if image.count <= 0 => Some(HashedStack::empty()),
        Some(image) if image.plain => Some(HashedStack::Actual {
            item_id: image.item_id?,
            count: image.count,
            components: HashedStackComponentHashes::empty(),
        }),
        _ => None,
    }
}
fn predicted_plain_carried(
    menu: &MenuReceipt,
    slot: usize,
    button: i8,
    mode: u8,
    limit: Option<i32>,
) -> Option<HashedStack> {
    let source = menu.image_slot(slot)?;
    let carried = menu.carried.as_ref();
    if mode == 1 {
        return plain_hashed_stack(carried);
    }
    if carried.is_none_or(|item| item.count <= 0) {
        if source.count <= 0 {
            return Some(HashedStack::empty());
        }
        let mut next = source.clone();
        if button == 1 {
            next.count = (next.count + 1) / 2;
        }
        return plain_hashed_stack(Some(&next));
    }
    let carried = carried?;
    if !source.plain || !carried.plain {
        return None;
    }
    if source.count <= 0 {
        let mut next = carried.clone();
        next.count = if button == 0 { 0 } else { next.count - 1 };
        return plain_hashed_stack(Some(&next));
    }
    if source.item_id != carried.item_id {
        return plain_hashed_stack(Some(source));
    }
    let limit = limit?;
    let room = (limit - source.count).max(0);
    let moved = room
        .min(carried.count)
        .min(if button == 1 { 1 } else { carried.count });
    let mut next = carried.clone();
    next.count -= moved;
    plain_hashed_stack(Some(&next))
}

fn increment_mechanic_receipt(map: &mut std::collections::BTreeMap<String, u64>, key: &str) {
    if let Some(count) = map.get_mut(key) {
        *count += 1;
    } else {
        map.insert(key.to_owned(), 1);
    }
}

impl Bot {
    fn mechanic_protected(&self, x: i32, y: i32, z: i32) -> bool {
        self.mechanics.as_ref().is_some_and(|shared| {
            let state = shared.lock();
            let position = pack_block_pos(x, y, z);
            state.positions.contains_key(&position) || state.container_positions.contains(&position)
        })
    }
}

impl Mechanics {
    fn predicted_carried(
        &self,
        index: usize,
        slot: usize,
        button: i8,
        mode: u8,
    ) -> Option<HashedStack> {
        let menu = &self.menus[index];
        let item = menu
            .image_slot(slot)?
            .item_id
            .or_else(|| menu.carried.as_ref().and_then(|carried| carried.item_id));
        let limit = item.and_then(|item| self.item_limits.get(&item).copied());
        predicted_plain_carried(menu, slot, button, mode, limit)
    }
}
