//! What a server keeps of a player from one stay in the world to the next, and what a
//! bot may expect of it when it joins again.
//!
//! A server that keeps a player's place puts them back where they left: in the same
//! spot, looking the same way, flying if they flew, with the same hotbar and the same
//! slot held (`docs/adr/0020-one-stay-per-player.md`, "What a player notices"). A bot
//! that is to judge that has to know what it had, and how much of it the server can be
//! held to:
//!
//! - [`Kept`] is what is kept, as a bot believes it or as a server said it on entering.
//! - [`World`] is what a server gives someone it has never seen, and the one case in
//!   which it does not give a place back.
//! - [`Left`] is what a bot had when its stay ended: the last state the server is known
//!   to have kept, and every state the bot was in after that. [`Left::fault`] is the
//!   rule.
//! - [`Journal`] is how a bot comes by a [`Left`] while it plays.

use std::collections::VecDeque;
use std::fmt::Write as _;

use clustine_data::items;
use clustine_protocol::item::ItemStack;

/// What a server keeps of a player across a leave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Kept {
    /// Where the player's feet are, as x, y and z.
    pub location: (f64, f64, f64),
    /// Yaw and pitch, in degrees.
    pub look: (f32, f32),
    pub flying: bool,
    /// The items in the hotbar, by slot from 0 to 8.
    pub hotbar: [Option<ItemStack>; 9],
    /// The hotbar slot that is held, from 0 to 8.
    pub selected_slot: i32,
}

/// What a server does with players it has no place for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct World {
    /// Where a player enters who was never in the world, looking ahead and not flying.
    pub spawn: (f64, f64, f64),
    /// What such a player holds, with the first slot selected.
    pub starting_hotbar: [Option<ItemStack>; 9],
    /// The height of the world's lowest block. A place whose feet are below it counts
    /// as no place.
    pub lowest_y: f64,
}

impl World {
    /// The flat world Clustine serves: players enter above the middle of the block at
    /// the origin (`spawn_point` in `bin/clustine/src/lib.rs`) with one each of nine
    /// blocks (`STARTING_HOTBAR` there), and the world's lowest block is at y = -64.
    pub fn clustine() -> Self {
        let starting = [
            items::STONE,
            items::COBBLESTONE,
            items::DIRT,
            items::GRASS_BLOCK,
            items::OAK_PLANKS,
            items::OAK_LOG,
            items::BRICKS,
            items::GLASS,
            items::GLOWSTONE,
        ];
        Self {
            spawn: (0.5, -60.0, 0.5),
            starting_hotbar: starting.map(|item| Some(ItemStack { item, count: 1 })),
            lowest_y: -64.0,
        }
    }

    /// What someone has who was never in the world.
    pub fn newcomer(&self) -> Kept {
        Kept {
            location: self.spawn,
            look: (0.0, 0.0),
            flying: false,
            hotbar: self.starting_hotbar,
            selected_slot: 0,
        }
    }

    /// What a player who left with `left` has when they enter again. It is `left`, with
    /// one exception: whoever left below the world's lowest block comes back at the
    /// spawn point, looking ahead and not flying, with what they held. The record has
    /// it so ("A place below the world counts as no place", section 4), because
    /// leaving and joining is the one way back for a player who fell through the
    /// floor.
    pub fn gives_back(&self, left: &Kept) -> Kept {
        if left.location.1 < self.lowest_y {
            Kept {
                hotbar: left.hotbar,
                selected_slot: left.selected_slot,
                ..self.newcomer()
            }
        } else {
            *left
        }
    }

    /// What is wrong with what a server said to someone who was never in the world,
    /// if anything is.
    pub fn newcomer_fault(&self, told: &Kept) -> Option<String> {
        let differences = differences(&self.newcomer(), told);
        (!differences.is_empty()).then(|| {
            format!(
                "someone who was never in the world enters at the spawn point, looking \
                 ahead, not flying, with the starting hotbar and its first slot held, \
                 but {}",
                differences.join("; ")
            )
        })
    }
}

/// What a bot had when a stay of it ended, as far as the server can be held to it.
///
/// **The rule for joining again** ([`Left::fault`]): what the server says on entering is
/// one of these states, as the world gives it back ([`World::gives_back`]). Not
/// necessarily the last one, and here is why a bound and not equality.
///
/// A region applies what a client sent in the order it was sent, and tells the world
/// store the player's place in the commit of every tick that changed anything of them
/// (the record's section 3: "Changed anything of a player who is in the region after
/// the tick ... `Has`"). What a tick produced is published only when the store has
/// confirmed the tick's commit ("Output commit", under "What can be built on"). So when
/// a bot reads the acknowledgement of an action, everything it sent before that action
/// was applied and is on disk: that state is [`Left::sure`], and "a later place is
/// never overwritten by an earlier one" (section 3), so nothing before it can come
/// back.
///
/// After that the record promises less. A stay that ends writes nothing ("Removed a
/// player (a leave, a replacement, a dead stay, an edge that is gone) | None. The
/// record has the last place written"), and of the place a second login finds, "What a
/// player notices" says: "At the place of the first one's last commit that the store
/// had taken when the login arrived. For a region that runs, that is behind the first
/// client's own screen by the ticks the region ran ahead (eight at most), by the
/// inputs it had not ticked yet and by the commits on their way: a few steps. For a
/// region that is slow, hung or without a worker it is behind by everything the first
/// edge still kept for it". There is no bound in blocks or in ticks in that, and a bot
/// cannot see how far a region ran ahead. What does hold in every one of those cases
/// is the order: the server stopped somewhere in what the bot sent, so the place is a
/// state the bot was in, after each thing it sent since the sure one
/// ([`Left::since`]), or the sure one itself.
///
/// A bot that leaves in good order waits for an acknowledgement of something it sent
/// after everything else and then sends nothing more. Then there is one state, and the
/// rule is equality.
#[derive(Debug, Clone, PartialEq)]
pub struct Left {
    /// The last state the server is known to have kept: what the bot had when it sent
    /// the latest action that was acknowledged, or what the server said on entering.
    pub sure: Kept,
    /// The state after each thing the bot sent since then, in the order it sent them.
    pub since: Vec<Kept>,
}

impl Left {
    /// For a bot that knows the server has all of `kept`.
    pub fn exactly(kept: Kept) -> Self {
        Self {
            sure: kept,
            since: Vec::new(),
        }
    }

    /// The last state the bot was in, which is what its own screen showed.
    pub fn last(&self) -> &Kept {
        self.since.last().unwrap_or(&self.sure)
    }

    /// What is wrong with what a server said on entering to a player who left with
    /// this, if anything is: `told` has to be one of the states, as `world` gives it
    /// back.
    pub fn fault(&self, world: &World, told: &Kept) -> Option<String> {
        let states = || std::iter::once(&self.sure).chain(&self.since);
        if states().any(|state| world.gives_back(state) == *told) {
            return None;
        }
        let from_sure = differences(&world.gives_back(&self.sure), told).join("; ");
        if self.since.is_empty() {
            return Some(format!("a player enters as they left, but {from_sure}"));
        }
        let mut fault = format!(
            "a player enters in one of the states they were in from the last the server \
             acknowledged to the last they sent, of which there are {}, and this is none \
             of them. Against the last acknowledged: {from_sure}. Against the last sent: \
             {}.",
            self.since.len() + 1,
            differences(&world.gives_back(self.last()), told).join("; ")
        );
        // A state that is none of them can still be made of parts of them: the place
        // of one with the hotbar of another. That is worth saying, since it is what a
        // server does that keeps the parts apart.
        let part = |same: &dyn Fn(&Kept, &Kept) -> bool| {
            states().any(|state| same(&world.gives_back(state), told))
        };
        let parts: [(&str, bool); 5] = [
            ("place", part(&|a, b| a.location == b.location)),
            ("look", part(&|a, b| a.look == b.look)),
            ("flying", part(&|a, b| a.flying == b.flying)),
            ("hotbar", part(&|a, b| a.hotbar == b.hotbar)),
            (
                "held slot",
                part(&|a, b| a.selected_slot == b.selected_slot),
            ),
        ];
        let never: Vec<&str> = (parts.iter())
            .filter(|(_, found)| !found)
            .map(|(name, _)| *name)
            .collect();
        if never.is_empty() {
            fault.push_str(" Each part of it is of one of them, but no one of them has all.");
        } else {
            let _ = write!(fault, " Of none of them: the {}.", never.join(", the "));
        }
        Some(fault)
    }
}

/// In what `told`, which a server said on entering, differs from `expected`: a clause
/// for each part that does.
fn differences(expected: &Kept, told: &Kept) -> Vec<String> {
    let mut differences = Vec::new();
    if told.location != expected.location {
        differences.push(format!(
            "the server put them at {:?} and not at {:?}",
            told.location, expected.location
        ));
    }
    if told.look != expected.look {
        differences.push(format!(
            "the server has them look along yaw and pitch {:?} and not {:?}",
            told.look, expected.look
        ));
    }
    if told.flying != expected.flying {
        differences.push(
            if told.flying {
                "the server has them fly, which they did not"
            } else {
                "the server does not have them fly, which they did"
            }
            .to_owned(),
        );
    }
    if told.selected_slot != expected.selected_slot {
        differences.push(format!(
            "the server says slot {} is held and not slot {}",
            told.selected_slot, expected.selected_slot
        ));
    }
    if told.hotbar != expected.hotbar {
        differences.push(format!(
            "the server says the hotbar holds the items {} and not {}",
            hotbar(&told.hotbar),
            hotbar(&expected.hotbar)
        ));
    }
    differences
}

/// A hotbar for a message: the id of the item in each slot, with its count if that is
/// not one, or a dash.
fn hotbar(slots: &[Option<ItemStack>; 9]) -> String {
    let slots: Vec<String> = slots
        .iter()
        .map(|slot| match slot {
            None => "-".to_owned(),
            Some(stack) if stack.count == 1 => stack.item.to_string(),
            Some(stack) => format!("{}x{}", stack.count, stack.item),
        })
        .collect();
    format!("[{}]", slots.join(" "))
}

/// What a bot was since the server last made sure of anything, kept while it plays so
/// that there is a [`Left`] when its stay ends, however it ends.
///
/// Every state goes in with the number of actions on blocks the bot had sent before
/// it. Those are what a server acknowledges, by their number, and an acknowledgement
/// makes sure of everything that was sent before the action it names: the first state
/// is always the sure one, and what an acknowledgement covers is dropped up to the
/// last state it covers. A bot that sends actions, or pulses, keeps a few states; one
/// that only walks keeps every step it ever took, which is why a bot remembers only
/// when asked to.
#[derive(Debug, Clone)]
pub(crate) struct Journal {
    states: VecDeque<(i32, Kept)>,
    /// Whether acknowledgements have stopped making anything sure; see
    /// [`Journal::hold`].
    held: bool,
}

impl Journal {
    /// For a bot that has sent nothing yet and was told `first` on entering.
    pub(crate) fn new(first: Kept) -> Self {
        Self {
            states: VecDeque::from([(0, first)]),
            held: false,
        }
    }

    /// Notes that the bot is as `now` after something it has just sent, having sent
    /// `actions` actions on blocks before that. Something that changed nothing adds no
    /// state: the server is in the same state whether or not it has applied it.
    pub(crate) fn sent(&mut self, actions: i32, now: Kept) {
        if self.states.back().is_none_or(|(_, last)| *last != now) {
            self.states.push_back((actions, now));
        }
    }

    /// Notes what the server said the player has: `change` puts it into a state. It
    /// goes into every state. What a server says it has committed, so none of the
    /// states can come back as they were before it; and a client takes the server's
    /// word over its own. (A bot that changes something before the server's word
    /// about it has arrived would have its change undone here. On entering, the
    /// server's word comes before the first chunk, and bots wait for that.)
    pub(crate) fn told(&mut self, change: impl Fn(&mut Kept)) {
        for (_, state) in &mut self.states {
            change(state);
        }
    }

    /// Notes that the server has acknowledged the action numbered `sequence`:
    /// everything sent before that action is applied and on disk.
    pub(crate) fn acknowledged(&mut self, sequence: i32) {
        if self.held {
            return;
        }
        while self
            .states
            .get(1)
            .is_some_and(|(actions, _)| *actions < sequence)
        {
            self.states.pop_front();
        }
    }

    /// From now on an acknowledgement makes nothing sure. A bot says so before it logs
    /// in a second time: once the store has the new stay's note the first stay is
    /// dead, and what it does then is applied and acknowledged "as before any kick",
    /// while "notes of D after the floor was raised are dropped" (the record's
    /// section 10, row 10). The first connection cannot tell which of its
    /// acknowledgements came after that moment, so none from here on counts.
    pub(crate) fn hold(&mut self) {
        self.held = true;
    }

    /// What the bot would have left with, were its stay to end now.
    pub(crate) fn left(&self) -> Left {
        let mut states = self.states.iter().map(|(_, state)| *state);
        Left {
            sure: states.next().expect("a journal always has a first state"),
            since: states.collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(item: i32) -> Option<ItemStack> {
        Some(ItemStack { item, count: 1 })
    }

    /// Someone on the ground at `x`, as a newcomer otherwise.
    fn at(x: f64) -> Kept {
        Kept {
            location: (x, -60.0, 0.5),
            ..World::clustine().newcomer()
        }
    }

    /// The fault of `told` for someone who left with `left`, which there has to be.
    fn fault_of(left: &Left, told: &Kept) -> String {
        left.fault(&World::clustine(), told)
            .expect("the rule finds fault")
    }

    #[test]
    fn someone_who_was_never_in_the_world_is_expected_at_the_spawn_point_with_the_starting_hotbar()
    {
        let world = World::clustine();
        let newcomer = world.newcomer();
        assert_eq!(newcomer.location, (0.5, -60.0, 0.5));
        assert_eq!(newcomer.hotbar[0], stack(items::STONE));
        assert_eq!(newcomer.hotbar[8], stack(items::GLOWSTONE));
        assert_eq!(world.newcomer_fault(&newcomer), None);

        // Each part by itself is a fault, and is named.
        let elsewhere = Kept {
            location: (40.5, -60.0, 0.5),
            ..newcomer
        };
        let fault = world.newcomer_fault(&elsewhere).unwrap();
        assert!(fault.contains("put them at (40.5, -60.0, 0.5) and not at (0.5, -60.0, 0.5)"));
        let mut emptied = newcomer;
        emptied.hotbar[3] = None;
        let fault = world.newcomer_fault(&emptied).unwrap();
        assert!(fault.contains("the hotbar holds"), "{fault}");
        for (changed, words) in [
            (
                Kept {
                    look: (90.0, 0.0),
                    ..newcomer
                },
                "look along",
            ),
            (
                Kept {
                    flying: true,
                    ..newcomer
                },
                "has them fly",
            ),
            (
                Kept {
                    selected_slot: 4,
                    ..newcomer
                },
                "slot 4 is held and not slot 0",
            ),
        ] {
            let fault = world.newcomer_fault(&changed).unwrap();
            assert!(fault.contains(words), "{fault}");
        }
    }

    #[test]
    fn a_player_who_left_in_good_order_is_expected_exactly_as_they_left() {
        let mut hotbar = World::clustine().starting_hotbar;
        hotbar[2] = stack(items::SANDSTONE);
        let had = Kept {
            location: (43.25, -48.0, -7.5),
            look: (135.0, 30.0),
            flying: true,
            hotbar,
            selected_slot: 2,
        };
        let left = Left::exactly(had);
        assert_eq!(left.fault(&World::clustine(), &had), None);

        // At the spawn point with the starting hotbar, as every join was once.
        let fault = fault_of(&left, &World::clustine().newcomer());
        assert!(
            fault.starts_with("a player enters as they left, but the server put them at"),
            "{fault}"
        );
        for words in [
            "look along yaw and pitch (0.0, 0.0) and not (135.0, 30.0)",
            "does not have them fly, which they did",
            "slot 0 is held and not slot 2",
            "the hotbar holds the items",
        ] {
            assert!(fault.contains(words), "{fault}");
        }

        // And each part by itself.
        for changed in [
            Kept {
                location: (43.25, -48.0, -7.0),
                ..had
            },
            Kept {
                look: (135.0, 0.0),
                ..had
            },
            Kept {
                flying: false,
                ..had
            },
            Kept {
                selected_slot: 3,
                ..had
            },
            Kept {
                hotbar: World::clustine().starting_hotbar,
                ..had
            },
        ] {
            assert!(left.fault(&World::clustine(), &changed).is_some());
        }
    }

    #[test]
    fn a_player_whose_connection_was_lost_may_enter_in_any_state_from_the_last_acknowledged_to_the_last_sent()
     {
        // Acknowledged at x = 10, then three steps, a slot selected, and a step more.
        let slot = |x: f64| Kept {
            selected_slot: 5,
            ..at(x)
        };
        let left = Left {
            sure: at(10.0),
            since: vec![at(10.3), at(10.6), at(10.9), slot(10.9), slot(11.2)],
        };
        let world = World::clustine();
        for state in [at(10.0), at(10.6), at(10.9), slot(10.9), slot(11.2)] {
            assert_eq!(left.fault(&world, &state), None, "{state:?}");
        }
        assert_eq!(left.last(), &slot(11.2));

        // Before what was acknowledged: the server went back on something it had.
        let fault = fault_of(&left, &at(9.7));
        assert!(
            fault.contains("of which there are 6, and this is none of them")
                && fault.contains("Of none of them: the place."),
            "{fault}"
        );
        // Beyond what was sent, and between two steps.
        assert!(left.fault(&world, &slot(11.5)).is_some());
        assert!(left.fault(&world, &at(10.45)).is_some());
        // The spawn point, as before places were kept.
        assert!(left.fault(&world, &world.newcomer()).is_some());
        // Parts of two states that were never one: the slot was selected at x = 10.9,
        // so nobody stood at x = 10.3 with it.
        let fault = fault_of(&left, &slot(10.3));
        assert!(
            fault.contains("Each part of it is of one of them, but no one of them has all."),
            "{fault}"
        );
    }

    #[test]
    fn a_player_who_left_below_the_world_is_expected_at_the_spawn_point_with_what_they_held() {
        let world = World::clustine();
        let mut hotbar = world.starting_hotbar;
        hotbar[0] = stack(items::BRICKS);
        let fallen = Kept {
            location: (300.5, -64.5, 12.5),
            look: (20.0, 80.0),
            flying: true,
            hotbar,
            selected_slot: 7,
        };
        let back = Kept {
            hotbar,
            selected_slot: 7,
            ..world.newcomer()
        };
        assert_eq!(world.gives_back(&fallen), back);
        let left = Left::exactly(fallen);
        assert_eq!(left.fault(&world, &back), None);
        // Not where they fell, and not with the starting hotbar.
        assert!(left.fault(&world, &fallen).is_some());
        assert!(left.fault(&world, &world.newcomer()).is_some());

        // Standing on the lowest block, or in a hole anywhere above, is a place.
        let low = Kept {
            location: (300.5, -64.0, 12.5),
            ..fallen
        };
        assert_eq!(world.gives_back(&low), low);
        assert!(Left::exactly(low).fault(&world, &back).is_some());
    }

    #[test]
    fn an_acknowledgement_makes_sure_of_what_was_sent_before_its_action_and_of_nothing_after() {
        let mut journal = Journal::new(at(0.5));
        // Two steps, the first action, two steps, the second action, a step.
        journal.sent(0, at(1.0));
        journal.sent(0, at(1.5));
        journal.sent(1, at(2.0));
        journal.sent(1, at(2.5));
        journal.sent(2, at(3.0));
        assert_eq!(journal.left().sure, at(0.5));
        assert_eq!(journal.left().since.len(), 5);

        // The first action was sent from x = 1.5; the steps after it are not covered.
        journal.acknowledged(1);
        let left = journal.left();
        assert_eq!(left.sure, at(1.5));
        assert_eq!(left.since, [at(2.0), at(2.5), at(3.0)]);
        // An acknowledgement that comes again, or late, takes nothing back.
        journal.acknowledged(1);
        journal.acknowledged(0);
        assert_eq!(journal.left(), left);

        journal.acknowledged(2);
        assert_eq!(journal.left().sure, at(2.5));
        assert_eq!(journal.left().since, [at(3.0)]);
        // A third action sent from where the bot stands covers everything.
        journal.acknowledged(3);
        assert_eq!(journal.left(), Left::exactly(at(3.0)));
    }

    #[test]
    fn what_changes_nothing_adds_no_state() {
        let mut journal = Journal::new(at(0.5));
        journal.sent(0, at(0.5));
        journal.sent(4, at(0.5));
        assert_eq!(journal.left(), Left::exactly(at(0.5)));
        journal.sent(4, at(1.0));
        journal.sent(5, at(1.0));
        assert_eq!(journal.left().since, [at(1.0)]);
    }

    #[test]
    fn nothing_is_made_sure_once_a_second_login_has_begun() {
        let mut journal = Journal::new(at(0.5));
        journal.sent(0, at(1.0));
        journal.acknowledged(1);
        journal.hold();
        journal.sent(1, at(1.5));
        journal.sent(2, at(2.0));
        // Acknowledged, but perhaps after the first stay was dead, when its place is
        // no longer written down.
        journal.acknowledged(2);
        journal.acknowledged(3);
        let left = journal.left();
        assert_eq!(left.sure, at(1.0));
        assert_eq!(left.since, [at(1.5), at(2.0)]);
        // So the second connection may find itself where the first was when it began
        // to log in again, which a journal that went on would call a fault.
        assert_eq!(left.fault(&World::clustine(), &at(1.0)), None);
    }

    #[test]
    fn what_the_server_says_the_player_has_is_so_in_every_state() {
        // The position comes first on entering, the hotbar a moment later.
        let mut empty = at(0.5);
        empty.hotbar = [None; 9];
        let mut journal = Journal::new(empty);
        journal.sent(
            0,
            Kept {
                location: (1.0, -60.0, 0.5),
                ..empty
            },
        );
        let hotbar = World::clustine().starting_hotbar;
        journal.told(|state| state.hotbar = hotbar);
        journal.told(|state| state.selected_slot = 6);
        let left = journal.left();
        let with = |x: f64| Kept {
            selected_slot: 6,
            ..at(x)
        };
        assert_eq!(left.sure, with(0.5));
        assert_eq!(left.since, [with(1.0)]);
        // An empty hand is then no state the player may come back in.
        assert!(left.fault(&World::clustine(), &empty).is_some());
    }
}
