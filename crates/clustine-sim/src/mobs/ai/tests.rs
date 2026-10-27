//! Tests of the AI from the brief's rules (sections 2 and 3): the selectors, the
//! controls, sensing, the goals and the navigation, on a world of their own.

mod goals;
mod navigation;
mod paths;
mod selection;
mod steps;

use std::collections::BTreeMap;

use clustine_data::{BlockState, EntityTypeInfo, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, ItemStack};

use super::{Ai, Setup, server_ai_step};
use crate::living::{Kind, Living, MAX_AIR};
use crate::mobs::Mob;
use crate::mobs::tick::{MobOutput, MobTick};
use crate::mobs::types::MobType;
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::body::Body;
use crate::physics::entities::Mover;
use crate::physics::item::Region as Holds;
use crate::physics::{self, Vec3, World};
use crate::region::Random;

/// The key of the mob the tests tick, which is even: its full selector ticks are those
/// whose game tick count is even.
const MOB_KEY: EntityId = EntityId(40);

/// A plot of land: stone below y = 0, air above, with what a test puts in it; the land
/// at and beyond x = `edge` is not held (neither land nor copy).
pub(crate) struct Plot {
    pub(super) blocks: BTreeMap<(i32, i32, i32), BlockState>,
    pub(super) edge: i32,
    pub(super) players: Vec<SeenPlayer>,
    pub(super) entities: Vec<(Entity, Aabb)>,
}

impl Plot {
    pub(crate) fn flat() -> Self {
        Self {
            blocks: BTreeMap::new(),
            edge: 1 << 20,
            players: Vec::new(),
            entities: Vec::new(),
        }
    }

    pub(super) fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        self.blocks.insert((x, y, z), state);
    }

    fn held(&self, x: i32) -> bool {
        x < self.edge
    }
}

impl World for Plot {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        if !self.held(pos.x) || pos.y < -64 || pos.y >= 320 {
            return None;
        }
        Some(match self.blocks.get(&(pos.x, pos.y, pos.z)) {
            Some(state) => *state,
            None if pos.y < 0 => blocks::STONE,
            None => blocks::AIR,
        })
    }

    fn has_chunk(&self, chunk: ChunkPos) -> bool {
        self.held(chunk.x * 16)
    }
}

impl Holds for Plot {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        -64
    }
}

impl MobWorld for Plot {
    fn is_loaded(&self, pos: BlockPos) -> bool {
        self.held(pos.x)
    }

    fn entities_in(&self, bounds: &Aabb) -> Vec<Seen<'_>> {
        self.entities
            .iter()
            .filter(|(_, own)| own.intersects(*bounds))
            .map(|(entity, own)| Seen {
                entity,
                bounds: *own,
                copied: false,
            })
            .collect()
    }

    fn entity(&self, key: EntityId) -> Option<Seen<'_>> {
        self.entities
            .iter()
            .find(|(entity, _)| entity.key == key)
            .map(|(entity, own)| Seen {
                entity,
                bounds: *own,
                copied: false,
            })
    }

    fn players(&self) -> Vec<SeenPlayer> {
        self.players.clone()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }
}

/// A player standing at `position`, holding `held` in the main hand.
pub(super) fn player(key: u64, position: Vec3, held: Option<i32>) -> SeenPlayer {
    SeenPlayer {
        key: EntityId(key),
        uuid: u128::from(key),
        position,
        eye_height: 1.62,
        yaw: 0.0,
        pitch: 0.0,
        main_hand: held.map(|item| ItemStack::new(item, 1)),
        off_hand: None,
        sneaking: false,
        creative: true,
        spectator: false,
        vehicle: None,
        moved_horizontally: false,
        sprinting: false,
        alive: true,
        sleeping: None,
        last_hurt_mob: None,
        last_hurt_by: None,
    }
}

/// A type whose goals a test gives.
pub(super) struct Built(pub(super) fn(&mut Setup));

impl MobType for Built {
    fn register_goals(&self, setup: &mut Setup) {
        (self.0)(setup);
    }
}

/// A cow's body at `position`, on the ground, at rest, with its fluids as the world has
/// them.
pub(super) fn cow_body<W: World + ?Sized>(world: &W, position: Vec3) -> Living {
    let info = EntityTypeInfo::of(entity_types::COW);
    let body = Body {
        position,
        motion: [0.0; 3],
        on_ground: true,
        fall_distance: 0.0,
        width: info.width,
        height: info.height,
        old_position: position,
        horizontal_collision: false,
        vertical_collision: false,
        vertical_collision_below: false,
        supporting: None,
        on_ground_no_blocks: false,
        stuck_speed: [0.0; 3],
        movements: Vec::new(),
        in_water: false,
        sync_position: false,
        falling_block: false,
        item: false,
        fell_on: None,
        noclip: None,
        mover: Mover {
            key: MOB_KEY,
            root: MOB_KEY,
            like_a_vehicle: false,
            happy_ghast: false,
        },
        steps: None,
    };
    let mut living = Living::new(
        body,
        Kind::Mob {
            kind: entity_types::COW as u16,
            hurts: Vec::new(),
        },
        info.eye_height,
    );
    living.first_tick = false;
    living.air = MAX_AIR;
    living.fluids = physics::fluids::update(world, &living.body.bounds());
    living.body.in_water = living.fluids.in_water();
    living
}

/// A cow of full health as its record would decode.
pub(super) fn healthy_cow() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = MAX_AIR;
    mob
}

/// One mob ticked again and again on a plot, as a region would tick it: the parts it
/// keeps between ticks are its record, its body as its record has it.
pub(crate) struct Ticked {
    pub(crate) mob: Mob,
    pub(super) living: Living,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(crate) lived: u32,
    pub(super) mob_type: &'static dyn MobType,
    pub(super) seed: u64,
}

impl Ticked {
    pub(crate) fn new(plot: &Plot, mob_type: &'static dyn MobType, at: Vec3) -> Self {
        Self {
            mob: healthy_cow(),
            living: cow_body(plot, at),
            yaw: 0.0,
            pitch: 0.0,
            lived: 0,
            mob_type,
            seed: 7,
        }
    }

    /// The mob's stream in its next tick.
    pub(crate) fn stream(&self) -> Random {
        Random::of_entity(self.seed, MOB_KEY, self.lived)
    }

    /// One AI step and the body's rotation after it; the draws it made.
    pub(super) fn step(&mut self, plot: &Plot) -> u64 {
        let mut draws = self.stream();
        let start = draws.clone();
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: MOB_KEY,
            kind: entity_types::COW as u16,
            lived: self.lived,
            tick_count: self.lived,

            vehicle: None,

            carries: false,

            leashed: false,

            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: self.pitch,
            mob_type: self.mob_type,
            world: plot,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        server_ai_step(&mut tick);
        super::rotate_body(&mut tick);
        self.mob = tick.mob;
        self.living = tick.living;
        self.yaw = tick.yaw;
        self.pitch = tick.pitch;
        self.lived += 1;
        let drawn = draws.draws_since(&start);
        self.living.body.old_position = self.living.body.position;
        drawn
    }

    /// A walker that goes exactly where its move control wants, its feet on the ground.
    pub(super) fn walk_to_wanted(&mut self, plot: &Plot) {
        let wanted = self.mob.body.controls.movement.wanted;
        self.living.body.position = wanted;
        self.living.fluids = physics::fluids::update(plot, &self.living.body.bounds());
    }

    pub(super) fn block(&self) -> BlockPos {
        physics::containing(self.living.body.position)
    }

    /// `f` run on the mob as its next AI step would have it, before its selectors.
    pub(crate) fn with_ai<R>(&mut self, plot: &Plot, f: impl FnOnce(&mut Ai<'_, '_>) -> R) -> R {
        let mut draws = self.stream();
        let mut tick = MobTick {
            sensed: None,
            ridden: Default::default(),
            key: MOB_KEY,
            kind: entity_types::COW as u16,
            lived: self.lived,
            tick_count: self.lived,

            vehicle: None,

            carries: false,

            leashed: false,

            counted: false,
            mob: self.mob.clone(),
            living: self.living.clone(),
            yaw: self.yaw,
            pitch: self.pitch,
            mob_type: self.mob_type,
            world: plot,
            draws: &mut draws,
            out: MobOutput::default(),
            controlled: None,
            controlled_by_mob: false,
        };
        let mut setup = Setup::default();
        self.mob_type.register_goals(&mut setup);
        let config = setup.config;
        let result = {
            let mut ai = Ai {
                tick: &mut tick,
                config: &config,
                sensing: Default::default(),
                navigation_ticked: false,
            };
            f(&mut ai)
        };
        self.mob = tick.mob;
        self.living = tick.living;
        result
    }
}
