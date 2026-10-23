//! Bounded ADR 0017 prototype: one declared, row-parallel system between
//! preparation and ordered writeback. No authority or mutable columns escape.

use super::{MoveWrite, RawIntent, ReadRow, StepCtx, step_entity};
use rayon::prelude::*;

type RowOutput = Option<(MoveWrite, Vec<RawIntent>)>;

/// Resource versions, not entity kinds. The store includes cold rows and slot
/// indexes: panic targeting can read a different entity's position.
#[repr(u8)]
#[derive(Clone, Copy)]
enum Resource {
    PreparedRows,
    EntityColumns,
    World,
    Registry,
    Targets,
    ClockAndRules,
    OrderedRowOutput,
}

const fn bit(resource: Resource) -> u16 {
    1 << resource as u8
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Barrier {
    Prepared,
    MotionWriteback,
}

struct Declaration {
    reads: u16,
    writes: u16,
    after: Barrier,
    before: Barrier,
    min_partition: usize,
}

/// The only registered node. Other tick phases remain explicit owner barriers;
/// they are NOT assumed independent because they are absent from this graph.
const ENTITY_STEP: Declaration = Declaration {
    reads: bit(Resource::PreparedRows)
        | bit(Resource::EntityColumns)
        | bit(Resource::World)
        | bit(Resource::Registry)
        | bit(Resource::Targets)
        | bit(Resource::ClockAndRules),
    writes: bit(Resource::OrderedRowOutput),
    after: Barrier::Prepared,
    before: Barrier::MotionWriteback,
    min_partition: 256,
};

const _: () = {
    // A row system may only return owned scratch, never write live authority.
    assert!(ENTITY_STEP.reads & ENTITY_STEP.writes == 0);
    assert!(ENTITY_STEP.writes == bit(Resource::OrderedRowOutput));
    assert!(matches!(ENTITY_STEP.after, Barrier::Prepared));
    assert!(matches!(ENTITY_STEP.before, Barrier::MotionWriteback));
    assert!(ENTITY_STEP.min_partition > 0);
};

/// Typed resource binding is the safety fence, not the declaration bitset:
/// StepCtx contains shared references only; the sole mutable borrow is output.
/// Indexed collection preserves dead-row holes and dense-slot merge order.
pub(super) fn run(ctx: &StepCtx<'_>, output: &mut Vec<RowOutput>) {
    #[cfg(test)]
    if tests::override_run(ctx, output) {
        return;
    }
    let parallel =
        rayon::current_num_threads() > 1 && ctx.rows.len() >= ENTITY_STEP.min_partition * 2;
    execute(ctx, output, parallel);
}

fn execute(ctx: &StepCtx<'_>, output: &mut Vec<RowOutput>, parallel: bool) {
    let step = |row: &ReadRow| row.alive.then(|| step_entity(*row, ctx));
    if parallel {
        ctx.rows
            .par_iter()
            .with_min_len(ENTITY_STEP.min_partition)
            .map(step)
            .collect_into_vec(output);
    } else {
        output.clear();
        output.extend(ctx.rows.iter().map(step));
    }
}

#[cfg(test)]
#[path = "system_graph_tests.rs"]
mod tests;
