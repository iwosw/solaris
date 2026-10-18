//! One transient admission allowance carried between dimensions of a global tick.
//! Previous-tick demand weights dimension shares. Each owner keeps a guaranteed
//! quarter of its dimension's share; unused shares return to the common remainder.
//! These are provisional limits, not performance guarantees.

use crate::dimensions::Dimension;

pub(crate) const DECISIONS_PER_TICK: usize = 256;
pub(crate) const PATH_NODES_PER_TICK: usize = 4_096;
const LIMITS: [usize; 2] = [DECISIONS_PER_TICK, PATH_NODES_PER_TICK];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Decision,
    PathNode,
}

impl Kind {
    const fn index(self) -> usize {
        match self {
            Self::Decision => 0,
            Self::PathNode => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Consumer {
    Allocator,
    Brain,
}

impl Consumer {
    const fn index(self) -> usize {
        match self {
            Self::Allocator => 0,
            Self::Brain => 1,
        }
    }
}

const fn dimension_index(dimension: Dimension) -> usize {
    match dimension {
        Dimension::Overworld => 0,
        Dimension::Nether => 1,
        Dimension::End => 2,
    }
}

type Demand = [[[u64; 2]; 2]; 3];

#[derive(Debug)]
pub(crate) struct Budget {
    tick: Option<u64>,
    reserved: [[[usize; 2]; 2]; 3],
    shared: [[usize; 2]; 3],
    common: [usize; 2],
    demand: Demand,
    announced: Demand,
    finished: [[bool; 2]; 3],
}

impl Default for Budget {
    fn default() -> Self {
        let mut budget = Self {
            tick: None,
            reserved: [[[0; 2]; 2]; 3],
            shared: [[0; 2]; 3],
            common: LIMITS,
            demand: [[[0; 2]; 2]; 3],
            announced: [[[0; 2]; 2]; 3],
            finished: [[false; 2]; 3],
        };
        budget.partition([[[1; 2]; 2]; 3]);
        budget
    }
}

impl Budget {
    /// Called at the global tick root, never at a dimension/brain entrypoint.
    /// Re-entering the same tick never refills an allowance or clears its demand.
    pub(crate) fn reset(&mut self, tick: u64) {
        if self.tick == Some(tick) {
            return;
        }
        let previous = if self.tick.is_none() {
            [[[1; 2]; 2]; 3]
        } else {
            std::array::from_fn(|dimension| {
                std::array::from_fn(|consumer| {
                    std::array::from_fn(|kind| {
                        self.demand[dimension][consumer][kind]
                            .max(self.announced[dimension][consumer][kind])
                    })
                })
            })
        };
        self.tick = Some(tick);
        self.demand = [[[0; 2]; 2]; 3];
        self.announced = [[[0; 2]; 2]; 3];
        self.finished = [[false; 2]; 3];
        self.partition(previous);
    }

    fn partition(&mut self, previous: Demand) {
        self.reserved = [[[0; 2]; 2]; 3];
        self.shared = [[0; 2]; 3];
        self.common = LIMITS;
        for (kind, limit) in LIMITS.into_iter().enumerate() {
            let weights: [u128; 3] = std::array::from_fn(|dimension| {
                previous[dimension]
                    .iter()
                    .map(|consumer| u128::from(consumer[kind]))
                    .sum()
            });
            let total: u128 = weights.iter().sum();
            if total == 0 {
                continue;
            }
            let active = weights.iter().filter(|weight| **weight != 0).count();
            // At least one unit per owner in every demanding dimension, even
            // when a huge earlier backlog would round its proportional share away.
            let proportional = limit - active * 2;
            for (dimension, weight) in weights.into_iter().enumerate() {
                if weight == 0 {
                    continue;
                }
                let share = 2 + (proportional as u128 * weight / total) as usize;
                let guarantee = (share / 4).max(1);
                for consumer in &mut self.reserved[dimension] {
                    consumer[kind] = guarantee;
                }
                self.shared[dimension][kind] = share - guarantee * 2;
                self.common[kind] -= share;
            }
        }
    }

    /// Announce a cheaply known full backlog without iterating denied work.
    /// Repeated snapshots do not inflate demand; attempted charges are counted
    /// separately and the greater of the two weights the following tick.
    pub(crate) fn note_demand(
        &mut self,
        dimension: Dimension,
        consumer: Consumer,
        kind: Kind,
        amount: usize,
    ) {
        let announced =
            &mut self.announced[dimension_index(dimension)][consumer.index()][kind.index()];
        *announced = (*announced).max(u64::try_from(amount).unwrap_or(u64::MAX));
    }

    /// Snapshot of units this tuple can currently charge, not another policy.
    pub(crate) fn allowance(&self, dimension: Dimension, consumer: Consumer, kind: Kind) -> usize {
        let dimension = dimension_index(dimension);
        let kind = kind.index();
        self.reserved[dimension][consumer.index()][kind]
            + self.shared[dimension][kind]
            + self.common[kind]
    }

    /// Charge atomically from this owner's guarantee, its dimension's shared
    /// allowance, then the common remainder. Refused attempts still count as
    /// demand for next tick, without spending any of the current allowance.
    pub(crate) fn try_charge(
        &mut self,
        dimension: Dimension,
        consumer: Consumer,
        kind: Kind,
        amount: usize,
    ) -> bool {
        let dimension = dimension_index(dimension);
        let consumer = consumer.index();
        let kind = kind.index();
        self.demand[dimension][consumer][kind] = self.demand[dimension][consumer][kind]
            .saturating_add(u64::try_from(amount).unwrap_or(u64::MAX));
        let own = self.reserved[dimension][consumer][kind];
        let shared = self.shared[dimension][kind];
        if amount > own + shared + self.common[kind] {
            return false;
        }
        let from_own = amount.min(own);
        self.reserved[dimension][consumer][kind] -= from_own;
        let rest = amount - from_own;
        let from_shared = rest.min(shared);
        self.shared[dimension][kind] -= from_shared;
        self.common[kind] -= rest - from_shared;
        true
    }

    /// Release a completed pass's unused guarantee. Once both owners in a
    /// dimension finish, its unused first-come share also becomes common.
    /// Late probes can still charge, but cannot reserve these units again.
    pub(crate) fn finish(&mut self, dimension: Dimension, consumer: Consumer) {
        let dimension = dimension_index(dimension);
        let consumer = consumer.index();
        if self.finished[dimension][consumer] {
            return;
        }
        self.finished[dimension][consumer] = true;
        for kind in 0..2 {
            self.common[kind] += std::mem::take(&mut self.reserved[dimension][consumer][kind]);
            if self.finished[dimension].iter().all(|finished| *finished) {
                self.common[kind] += std::mem::take(&mut self.shared[dimension][kind]);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn remaining(&self, kind: Kind) -> usize {
        let kind = kind.index();
        self.common[kind]
            + self.shared.iter().map(|share| share[kind]).sum::<usize>()
            + self
                .reserved
                .iter()
                .flatten()
                .map(|share| share[kind])
                .sum::<usize>()
    }

    #[cfg(test)]
    pub(crate) fn exhaust_for_test(&mut self, kind: Kind) {
        for dimension in Dimension::ALL {
            for consumer in [Consumer::Allocator, Consumer::Brain] {
                let amount = self.allowance(dimension, consumer, kind);
                assert!(self.try_charge(dimension, consumer, kind, amount));
            }
        }
        assert_eq!(self.remaining(kind), 0);
    }
}

#[cfg(test)]
#[path = "tick_budget_tests.rs"]
mod tests;
