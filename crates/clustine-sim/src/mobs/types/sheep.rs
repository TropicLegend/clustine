//! Sheep (`Sheep`, `SheepColorSpawnRules`, `EatBlockGoal`, `DyeItem`;
//! `docs/groundwork/n7-brief.md`, sections 4.10 and 2.10): an animal of wheat whose wool
//! is of a colour the biome it is made in draws, shorn with shears for its table's wool,
//! grown again by eating grass, dyed by a dye, and mixed at breeding as the crafting of
//! two dyes mixes them.
//!
//! Named difference: `mobGriefing` is the game's default, on.

use clustine_blocks::data::in_tag;
use clustine_data::{BlockState, blocks, items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, biome_in_tag};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, Items, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal, RandomStrollGoal,
    TemptGoal,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup, navigation};
use crate::mobs::body::hurt::{DamageSource, GameEvent};
use crate::mobs::body::{Event, loot};
use crate::mobs::tick::{Local, MobTick};
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::{BlockPos, ItemStack};

/// `#sheep_food`.
const FOOD: Items = Items::Tag("minecraft:sheep_food");

/// A lamb's size.
const BABY: Dimensions = Dimensions {
    width: 0.45,
    height: 0.65,
    eye_height: 0.656_25,
};

/// The synced data's index of its wool (`DATA_WOOL_ID`).
const WOOL_INDEX: u8 = 18;

/// The wool byte's bit of a sheared sheep; its low four bits are the colour.
const SHEARED: u8 = 0x10;

/// The dye colours by their ids (`DyeColor`).
pub(crate) const COLOURS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// A colour's id by its name.
const fn colour(name: &str) -> u8 {
    let mut at = 0;
    while at < COLOURS.len() {
        if same(COLOURS[at], name) {
            return at as u8;
        }
        at += 1;
    }
    0
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut at = 0;
    while at < a.len() {
        if a[at] != b[at] {
            return false;
        }
        at += 1;
    }
    true
}

const WHITE: u8 = colour("white");
const ORANGE: u8 = colour("orange");
const MAGENTA: u8 = colour("magenta");
const LIGHT_BLUE: u8 = colour("light_blue");
const YELLOW: u8 = colour("yellow");
const LIME: u8 = colour("lime");
const PINK: u8 = colour("pink");
const GRAY: u8 = colour("gray");
const LIGHT_GRAY: u8 = colour("light_gray");
const CYAN: u8 = colour("cyan");
const PURPLE: u8 = colour("purple");
const BLUE: u8 = colour("blue");
const BROWN: u8 = colour("brown");
const GREEN: u8 = colour("green");
const RED: u8 = colour("red");
const BLACK: u8 = colour("black");

/// What a sheep keeps beyond an animal: its wool byte, as the game syncs it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SheepState {
    pub(crate) wool: u8,
}

impl SheepState {
    /// `getColor()`: an id out of range is white.
    pub(crate) fn colour(&self) -> u8 {
        let id = self.wool & 0x0F;
        if usize::from(id) < COLOURS.len() {
            id
        } else {
            WHITE
        }
    }

    pub(crate) fn sheared(&self) -> bool {
        self.wool & SHEARED != 0
    }

    pub(crate) fn set_colour(&mut self, colour: u8) {
        self.wool = (self.wool & 0xF0) | (colour & 0x0F);
    }

    pub(crate) fn set_sheared(&mut self, sheared: bool) {
        if sheared {
            self.wool |= SHEARED;
        } else {
            self.wool &= !SHEARED;
        }
    }
}

impl Own for SheepState {}

/// The sheep.
pub(crate) struct Sheep;

/// `SheepColorSpawnRules.getSheepColor(biome, random)`: one of the biome's few colours,
/// else its common one, and one in 500 of those pink.
pub(crate) fn spawn_colour(biome: Option<&str>, level: &mut Random) -> u8 {
    let (few, common) = if biome_in_tag(biome, "minecraft:spawns_warm_variant_farm_animals") {
        ([(GRAY, 5), (LIGHT_GRAY, 5), (WHITE, 5), (BLACK, 3)], BROWN)
    } else if biome_in_tag(biome, "minecraft:spawns_cold_variant_farm_animals") {
        ([(LIGHT_GRAY, 5), (GRAY, 5), (WHITE, 5), (BROWN, 3)], BLACK)
    } else {
        ([(BLACK, 5), (GRAY, 5), (LIGHT_GRAY, 5), (BROWN, 3)], WHITE)
    };
    let mut drawn = level.next_int(100) as i32;
    for (colour, weight) in few {
        drawn -= weight;
        if drawn < 0 {
            return colour;
        }
    }
    if level.next_int(500) < 499 {
        common
    } else {
        PINK
    }
}

/// `DyeColor.getMixedColor(level, a, b)`: what the crafting of the two dyes makes (the
/// nine recipes of two dyes, shapeless), else one of the two by the level's random.
pub(crate) fn mixed_colour(a: u8, b: u8, level: &mut Random) -> u8 {
    const MIXES: [(u8, u8, u8); 9] = [
        (BLACK, WHITE, GRAY),
        (GRAY, WHITE, LIGHT_GRAY),
        (BLUE, WHITE, LIGHT_BLUE),
        (GREEN, WHITE, LIME),
        (RED, WHITE, PINK),
        (BLUE, GREEN, CYAN),
        (BLUE, RED, PURPLE),
        (RED, YELLOW, ORANGE),
        (PURPLE, PINK, MAGENTA),
    ];
    for (one, other, made) in MIXES {
        if (a, b) == (one, other) || (a, b) == (other, one) {
            return made;
        }
    }
    if level.next_bool() { a } else { b }
}

/// The colour a dye item gives (`DataComponents.DYE`), by its name.
pub(crate) fn dye_colour(item: i32) -> Option<u8> {
    let name = clustine_data::ITEMS.get(usize::try_from(item).ok()?)?.name;
    let colour = name.strip_prefix("minecraft:")?.strip_suffix("_dye")?;
    COLOURS
        .iter()
        .position(|known| *known == colour)
        .map(|at| at as u8)
}

/// What its loot tables read of it: its colour and whether it is sheared.
fn facts(own: &SheepState) -> loot::Facts {
    loot::Facts {
        sheep_colour: Some(COLOURS[usize::from(own.colour())]),
        sheared: Some(own.sheared()),
        ..loot::Facts::default()
    }
}

/// `Sheep.ate()`: `Mob.ate`'s game event, its wool back, and a lamb a minute older.
fn ate(tick: &mut MobTick<'_>) {
    tick.out.events.push(Event::GameEvent(GameEvent::Eat));
    let now = tick.lived;
    let (mut animal, mut own) = animal::read::<SheepState>(&tick.mob.state);
    own.set_sheared(false);
    if animal.can_age_up() {
        animal.age_up(60, false, now);
    }
    tick.mob.state = animal::write(&animal, &own);
}

/// `EatBlockGoal`'s state: what is left of its eating.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Eating {
    pub(crate) tick: i32,
}

/// `EatBlockGoal(mob)`: now and then a sheep stops to eat the grass it stands in, or the
/// grass block it stands on, and its wool grows back.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EatBlockGoal;

/// `#edible_for_sheep`.
fn edible(state: BlockState) -> bool {
    in_tag(state, "minecraft:edible_for_sheep")
}

impl Goal for EatBlockGoal {
    type State = Eating;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK | flags::JUMP
    }

    fn can_use(&self, _: &mut Eating, ai: &mut Ai<'_, '_>) -> bool {
        let chance = if animal::is_baby(&ai.tick.mob) {
            50
        } else {
            1000
        };
        let bound = self.adjusted_tick_delay(chance).max(1) as u32;
        if ai.tick.draws.next_int(bound) != 0 {
            return false;
        }
        let at = ai.block_position();
        let world = ai.tick.world;
        edible(world.state(at)) || animal::is_grass_block(world.state(below(at)))
    }

    fn can_continue(&self, state: &mut Eating, _: &mut Ai<'_, '_>) -> bool {
        state.tick > 0
    }

    fn start(&self, state: &mut Eating, ai: &mut Ai<'_, '_>) {
        state.tick = self.adjusted_tick_delay(40);
        ai.tick.out.events.push(Event::Entity(10));
        navigation::stop(ai);
    }

    fn stop(&self, state: &mut Eating, _: &mut Ai<'_, '_>) {
        state.tick = 0;
    }

    fn tick(&self, state: &mut Eating, ai: &mut Ai<'_, '_>) {
        state.tick = (state.tick - 1).max(0);
        if state.tick != self.adjusted_tick_delay(4) {
            return;
        }
        let at = ai.block_position();
        let world = ai.tick.world;
        if edible(world.state(at)) {
            ai.tick.out.local.push(Local::DestroyBlock { pos: at });
            ate(ai.tick);
        } else if animal::is_grass_block(world.state(below(at))) {
            let grass = below(at);
            ai.tick.out.local.push(Local::LevelEvent {
                kind: 2001,
                pos: grass,
                data: i32::from(blocks::GRASS_BLOCK.0),
            });
            ai.tick.out.local.push(Local::SetBlock {
                pos: grass,
                state: blocks::DIRT,
                flags: 2,
            });
            ate(ai.tick);
        }
    }
}

fn below(pos: BlockPos) -> BlockPos {
    BlockPos::new(pos.x, pos.y - 1, pos.z)
}

/// `Sheep.shear(level, PLAYERS, shears)`: its sound, then its shearing table's wool
/// drawn before it is sheared (the table asks that it is not), each one an item a block
/// above it pushed a little by five of its draws; then sheared.
fn shear(used: &mut Used<'_>, own: &mut SheepState) {
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_SHEEP_SHEAR,
        source: animal::SOUND_SOURCE_PLAYERS,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    let baby = animal::is_baby(used.mob);
    let drops = loot::named_drops(
        "minecraft:shearing/sheep",
        used.kind,
        baby,
        facts(own),
        used.draws,
    );
    let [x, y, z] = used.position;
    for stack in drops {
        for _ in 0..stack.count {
            let push = [
                f64::from((used.draws.next_float() - used.draws.next_float()) * 0.1),
                f64::from(used.draws.next_float() * 0.05),
                f64::from((used.draws.next_float() - used.draws.next_float()) * 0.1),
            ];
            used.did.push(Did::Drop {
                stack: ItemStack {
                    item: stack.item,
                    count: 1,
                },
                at: [x, y + 1.0, z],
                pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                push: Some(push),
            });
        }
    }
    own.set_sheared(true);
}

impl MobType for Sheep {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, PanicGoal { speed: 1.25 });
        setup.add_goal(2, animal::BreedGoal { speed: 1.0 });
        setup.add_goal(3, TemptGoal::new(1.1, FOOD, false));
        setup.add_goal(4, animal::FollowParentGoal { speed: 1.1 });
        setup.add_goal(5, EatBlockGoal);
        setup.add_goal(6, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(7, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(8, RandomLookAroundGoal);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, blocks::GRASS_BLOCK)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Sheep.mobInteract`: shears shear a sheep ready for it and do nothing else to
    /// one that is not; anything else is an animal's use.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if animal::is_item(used.held, items::SHEARS) {
            let (animal, mut own) = animal::read::<SheepState>(&used.mob.state);
            if own.sheared() || animal.is_baby() {
                return Interaction::Consume;
            }
            shear(used, &mut own);
            used.mob.state = animal::write(&animal, &own);
            used.did.push(Did::GameEvent("minecraft:shear"));
            return Interaction::SuccessServer;
        }
        animal::mob_interact(used, FOOD, None)
    }

    /// `DyeItem.interactLivingEntity`: a dye of another colour on a living sheep with its
    /// wool dyes it, heard by all but the player.
    fn item_interact(&self, used: &mut Used<'_>) -> Interaction {
        let Some(dye) = used
            .held
            .filter(|stack| stack.count > 0)
            .and_then(|stack| dye_colour(stack.item))
        else {
            return Interaction::Pass;
        };
        let (animal, mut own) = animal::read::<SheepState>(&used.mob.state);
        if used.mob.body.vitals.dying() || own.sheared() || own.colour() == dye {
            return Interaction::Pass;
        }
        used.did.push(Did::Sound {
            sound: sounds::ITEM_DYE_USE,
            source: animal::SOUND_SOURCE_PLAYERS,
            at: used.position,
            volume: 1.0,
            pitch: 1.0,
            but_the_player: true,
        });
        own.set_colour(dye);
        used.mob.state = animal::write(&animal, &own);
        Interaction::Success
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let (animal, mut own) = animal::read::<SheepState>(&spawn.mob.state);
        own.set_colour(spawn_colour(spawn.biome, spawn.level));
        spawn.mob.state = animal::write(&animal, &own);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = animal::read::<SheepState>(&breeding.this.state);
        let (_, partner) = animal::read::<SheepState>(&breeding.partner.state);
        let mut own = SheepState::default();
        own.set_colour(mixed_colour(
            this.colour(),
            partner.colour(),
            breeding.draws,
        ));
        let mut child = Mob::fresh();
        child.state = animal::write(&animal::Animal::default(), &own);
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SHEEP_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SHEEP_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SHEEP_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = animal::read::<SheepState>(&mob.state);
        let mut values = animal::shown(mob);
        values.push((WOOL_INDEX, DataShown::Byte(own.wool), own.wool == 0));
        values
    }

    fn loot_facts(&self, mob: &Mob) -> loot::Facts {
        facts(&animal::read::<SheepState>(&mob.state).1)
    }
}
