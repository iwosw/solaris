// Visibility is always per observer. Only explicitly selected convergence
// probes retain per-observer positions; all other bodies stay process-wide.
impl EntityReceipt {
    fn visible_to(&self, bot: u32) -> bool {
        self.visibility[bot as usize / 64] & (1_u64 << (bot % 64)) != 0
    }
    fn show(&mut self, bot: u32) -> bool {
        let word = &mut self.visibility[bot as usize / 64];
        let bit = 1_u64 << (bot % 64);
        if *word & bit != 0 {
            return false;
        }
        *word |= bit;
        self.visible_observers += 1;
        self.active = true;
        true
    }
    fn hide(&mut self, bot: u32) -> bool {
        let word = &mut self.visibility[bot as usize / 64];
        let bit = 1_u64 << (bot % 64);
        if *word & bit == 0 {
            return false;
        }
        *word &= !bit;
        self.visible_observers -= 1;
        if let Some(position) = self.reader_positions.get_mut(bot as usize) {
            *position = None;
        }
        self.active = self.visible_observers > 0;
        true
    }
}
fn initial_entity_visibility(clients: usize, bot: u32) -> Vec<u64> {
    let mut visibility = vec![0; clients.div_ceil(64)];
    visibility[bot as usize / 64] = 1_u64 << (bot % 64);
    visibility
}
