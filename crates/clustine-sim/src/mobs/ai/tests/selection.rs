//! The goal selector's rules (the brief's 2.2): order, priorities and locks, and the
//! full selection every other tick.

use super::super::selector::{Goal, flags};
use super::super::{Ai, Setup};
use super::{Built, Plot, Ticked};

/// A goal that writes what is asked of it into its mob's type state (which a goal may
/// read and write), as its code and the event: `u` asked to start, `c` asked to go on,
/// `s` started, `x` stopped, `t` ticked.
#[derive(Debug, Clone, Copy)]
struct Probe {
    code: u8,
    flags: u8,
    every_tick: bool,
}

impl Probe {
    const fn new(code: u8, flags: u8) -> Self {
        Self {
            code,
            flags,
            every_tick: false,
        }
    }

    fn note(&self, ai: &mut Ai<'_, '_>, event: u8) {
        ai.tick.mob.state.bytes.extend([self.code, event]);
    }
}

impl Goal for Probe {
    type State = ();

    fn flags(&self) -> u8 {
        self.flags
    }

    fn every_tick(&self) -> bool {
        self.every_tick
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        self.note(ai, b'u');
        true
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        self.note(ai, b'c');
        true
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        self.note(ai, b's');
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        self.note(ai, b'x');
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        self.note(ai, b't');
    }
}

/// What the probes wrote in the step, as text.
fn log(ticked: &mut Ticked) -> String {
    String::from_utf8(std::mem::take(&mut ticked.mob.state.bytes)).unwrap()
}

static ORDERED: Built = Built(|setup: &mut Setup| {
    setup.add_goal(2, Probe::new(b'A', flags::MOVE));
    setup.add_goal(1, Probe::new(b'B', flags::MOVE));
    setup.add_goal(3, Probe::new(b'C', flags::LOOK));
    setup.add_goal(1, Probe::new(b'D', flags::MOVE));
});

#[test]
fn goals_are_tried_in_the_order_added_and_a_lower_number_takes_a_flag_from_a_higher() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &ORDERED, [0.5, 0.0, 0.5]);
    cow.step(&plot);
    // A starts, then B, added after it with a lower number, takes MOVE from it in the
    // same pass: A is started and stopped and never ticked. D's number is B's, which
    // never displaces it: D is not even asked. The running goals tick in the order
    // added.
    assert_eq!(log(&mut cow), "AuAsBuAxBsCuCsBtCt");
    // The next full tick asks the running goals to go on, in order, and nothing else
    // can take their flags.
    cow.step(&plot);
    cow.step(&plot);
    assert_eq!(log(&mut cow), "BcCcBtCt");
}

static DISPLACED: Built = Built(|setup: &mut Setup| {
    setup.add_goal(3, Probe::new(b'A', flags::MOVE | flags::LOOK));
    setup.add_goal(2, Probe::new(b'B', flags::MOVE));
    setup.add_goal(4, Probe::new(b'C', flags::LOOK));
    setup.add_goal(2, Probe::new(b'E', flags::LOOK));
});

#[test]
fn a_displaced_goal_keeps_its_other_flags_until_the_next_clean_up() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &DISPLACED, [0.5, 0.0, 0.5]);
    cow.step(&plot);
    // B takes MOVE from A; LOOK is still locked to A, no longer running, for the rest of
    // the pass: C (4) is compared with A's 3 and not asked, E (2) is and takes it.
    assert_eq!(log(&mut cow), "AuAsBuAxBsEuEsBtEt");
    let locks = &cow.mob.mind.goals.0.locks;
    assert_eq!(locks[0], Some(1));
    assert_eq!(locks[1], Some(3));
}

static HALVES: Built = Built(|setup: &mut Setup| {
    setup.add_goal(0, Probe::new(b'E', flags::LOOK));
    setup.add_goal(
        0,
        Probe {
            every_tick: true,
            ..Probe::new(b'F', flags::JUMP)
        },
    );
});

#[test]
fn the_full_selection_runs_only_every_other_tick_and_the_every_tick_goals_between() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &HALVES, [0.5, 0.0, 0.5]);
    let mut logs = Vec::new();
    for _ in 0..6 {
        cow.step(&plot);
        logs.push(log(&mut cow));
    }
    // The key is even: the game's tick counts 1 (at most 1, always full), 2, 4 and 6
    // are full; 3 and 5 tick only the goal ticked every tick.
    assert_eq!(
        logs,
        [
            "EuEsFuFsEtFt",
            "EcFcEtFt",
            "Ft",
            "EcFcEtFt",
            "Ft",
            "EcFcEtFt",
        ]
    );
}

static EMPTY: Built = Built(|_: &mut Setup| {});

#[test]
fn a_mob_whose_type_no_longer_adds_a_goal_drops_its_part() {
    let plot = Plot::flat();
    let mut cow = Ticked::new(&plot, &ORDERED, [0.5, 0.0, 0.5]);
    cow.step(&plot);
    assert_eq!(cow.mob.mind.goals.0.running, [1, 2]);
    cow.mob_type = &EMPTY;
    cow.step(&plot);
    assert!(cow.mob.mind.goals.0.running.is_empty());
    assert_eq!(cow.mob.mind.goals.0.locks, [None; 4]);
}
