//! One fresh, reusable spatial census per redstone pass. Balanced AABB subtrees
//! prune distant bodies without scanning the population for each control.
use crate::entities::EntitySlot;
use crate::tick::TickState;

#[derive(Clone, Copy)]
struct Body {
    slot: EntitySlot,
    min: [f64; 3],
    max: [f64; 3],
    subtree_min: [f64; 3],
    subtree_max: [f64; 3],
}

#[derive(Default)]
pub(super) struct EntityQuery {
    bodies: Vec<Body>,
    #[cfg(test)]
    pub(super) visited: std::cell::Cell<usize>,
}

impl EntityQuery {
    pub(super) fn rebuild(&mut self, state: &TickState) {
        self.bodies.clear();
        for slot in state.entities.slots() {
            let body = state.entities.body(slot);
            if !body.alive
                || state
                    .players
                    .get(&body.id)
                    .is_some_and(|player| player.spectator())
            {
                continue;
            }
            let pos = state.entities.pos(slot);
            let min = [pos[0] - body.half_width, pos[1], pos[2] - body.half_width];
            let max = [
                pos[0] + body.half_width,
                pos[1] + body.height,
                pos[2] + body.half_width,
            ];
            if (0..3).any(|axis| {
                !min[axis].is_finite() || !max[axis].is_finite() || min[axis] > max[axis]
            }) {
                continue;
            }
            self.bodies.push(Body {
                slot,
                min,
                max,
                subtree_min: min,
                subtree_max: max,
            });
        }
        Self::partition(&mut self.bodies, 0);
        #[cfg(test)]
        self.visited.set(0);
    }

    fn partition(bodies: &mut [Body], axis: usize) -> Option<([f64; 3], [f64; 3])> {
        if bodies.is_empty() {
            return None;
        }
        let middle = bodies.len() / 2;
        let (left, root, right) = bodies.select_nth_unstable_by(middle, |a, b| {
            a.min[axis]
                .total_cmp(&b.min[axis])
                .then_with(|| a.slot.cmp(&b.slot))
        });
        let mut min = root.min;
        let mut max = root.max;
        for bounds in [
            Self::partition(left, (axis + 1) % 3),
            Self::partition(right, (axis + 1) % 3),
        ]
        .into_iter()
        .flatten()
        {
            for axis in 0..3 {
                min[axis] = min[axis].min(bounds.0[axis]);
                max[axis] = max[axis].max(bounds.1[axis]);
            }
        }
        root.subtree_min = min;
        root.subtree_max = max;
        Some((min, max))
    }

    pub(super) fn count(
        &self,
        min: [f64; 3],
        max: [f64; 3],
        limit: usize,
        accepts: impl Fn(EntitySlot) -> bool,
    ) -> usize {
        self.count_in(&self.bodies, min, max, limit, &accepts)
    }

    fn count_in(
        &self,
        bodies: &[Body],
        min: [f64; 3],
        max: [f64; 3],
        limit: usize,
        accepts: &impl Fn(EntitySlot) -> bool,
    ) -> usize {
        if bodies.is_empty() || limit == 0 {
            return 0;
        }
        let middle = bodies.len() / 2;
        let root = &bodies[middle];
        #[cfg(test)]
        self.visited.set(self.visited.get() + 1);
        let overlaps = |lo: [f64; 3], hi: [f64; 3]| {
            (0..3).all(|axis| hi[axis] > min[axis] && lo[axis] < max[axis])
        };
        if !overlaps(root.subtree_min, root.subtree_max) {
            return 0;
        }
        let mut count = usize::from(overlaps(root.min, root.max) && accepts(root.slot));
        count += self.count_in(&bodies[..middle], min, max, limit - count, accepts);
        count += self.count_in(&bodies[middle + 1..], min, max, limit - count, accepts);
        count
    }
}
