//! Memories that run out (the brief's 7.3.12 and 7.3.23): `CountDownCooldownTicks`,
//! which lowers an integer memory by one a tick, and `EraseMemoryIf`.

use super::super::{Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed};

/// `CountDownCooldownTicks(memory)` (7.3.12): while the memory is above nothing, one off
/// it each tick; erased the tick after it reaches nothing. The memory is a
/// [`super::super::Countdown`]: its lowering begins at this behaviour's start
/// (`begin_lowering`) and each tick says only that this tick's lowering has come
/// (`lower`), so that a cooldown that counts down writes nothing a tick (the plan,
/// section 2.2 item 3).
pub(crate) struct CountDownCooldownTicks {
    entry: [(MemoryKind, Status); 1],
}

/// [`CountDownCooldownTicks`] of `memory`, boxed.
pub(crate) fn count_down(memory: MemoryKind) -> Box<dyn Behaviour> {
    debug_assert!(memory.counts_down(), "{} is not a countdown", memory.name());
    Box::new(HeldTimed::new(CountDownCooldownTicks {
        entry: [(memory, Status::Present)],
    }))
}

impl CountDownCooldownTicks {
    fn memory(&self) -> MemoryKind {
        self.entry[0].0
    }
}

impl Timed for CountDownCooldownTicks {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &self.entry
    }

    // It runs for as long as it can be used.
    fn timed_out(&self, _: &(), _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories.int(self.memory()).is_some_and(|left| left > 0)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.begin_lowering(self.memory());
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.lower(self.memory());
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(self.memory());
    }
}

/// `EraseMemoryIf.create(predicate, memory)` (7.3.23): the memory, where it is there and
/// the predicate holds, erased.
pub(crate) struct EraseMemoryIf {
    pub(crate) predicate: fn(&mut Cx<'_, '_, '_>) -> bool,
    memory: [MemoryKind; 1],
}

/// [`EraseMemoryIf`], boxed.
pub(crate) fn erase_if(
    predicate: fn(&mut Cx<'_, '_, '_>) -> bool,
    memory: MemoryKind,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(EraseMemoryIf {
        predicate,
        memory: [memory],
    }))
}

impl Shot for EraseMemoryIf {
    fn required(&self) -> &[MemoryKind] {
        &self.memory
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let memory = self.memory[0];
        if !cx.memories.check(memory, Status::Present) || !(self.predicate)(cx) {
            return false;
        }
        cx.memories.erase(memory);
        true
    }
}
