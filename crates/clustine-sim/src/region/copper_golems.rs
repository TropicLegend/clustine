//! What the copper golem does to blocks (`docs/groundwork/n7-brief.md`, sections 7.0.5
//! and 7.13.7; `docs/groundwork/n7j-plan.md`, section 3, item 9): a carved pumpkin or a
//! jack o'lantern placed on a block of copper builds one where the pumpkin was, with a
//! copper chest of the block's age where the copper was; a golem with a chest open counts
//! as one of its openers at the chest's recheck; and its statue turns to its next pose
//! when used.
//!
//! At a line: a golem is built only where the pumpkin and the copper are both the
//! region's own, and counts only at the chests of its region's land (the plan's section
//! 3, item 9).
//!
//! Named differences: only the upright pattern (the pumpkin on top) builds a golem
//! (`BlockPattern.find`, which may match the pattern turned about, is not in the dumps),
//! and its chest stays single beside another copper chest (`getChestType` is not
//! ported); no advancement is given for building one.

use clustine_blocks::data::{in_tag, is_block, is_class, relative, same_block, set};
use clustine_blocks::growth::chest_connected_direction;
use clustine_data::{BLOCKS, BlockClass, BlockState, EntityTypeInfo, blocks, entity_types, sounds};
use clustine_world::{BlockPos, PlayerId};

use super::Region;
use super::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::mobs::Mob;
use crate::mobs::types::copper_golem::{self, CONTAINER_INTERACTION_RANGE, STAGES};
use crate::physics::aabb::Aabb;

/// `PUMPKINS_PREDICATE`: a carved pumpkin or a jack o'lantern.
fn is_pumpkin(state: BlockState) -> bool {
    is_block(state, blocks::CARVED_PUMPKIN) || is_block(state, blocks::JACK_O_LANTERN)
}

/// The weathering of a block of `#copper` and whether it is waxed: its age as
/// `WeatheringCopper.getAge` has it, a waxed block's that of the block its wax comes off
/// to (`HoneycombItem.WAX_OFF_BY_BLOCK`). Unaffected for anything else.
pub(crate) fn copper_age(state: BlockState) -> (u8, bool) {
    let Some(name) = state
        .block()
        .and_then(|block| block.name.strip_prefix("minecraft:"))
    else {
        return (copper_golem::UNAFFECTED, false);
    };
    let (waxed, name) = match name.strip_prefix("waxed_") {
        Some(rest) => (true, rest),
        None => (false, name),
    };
    let stage = (1..STAGES.len())
        .rev()
        .find(|stage| name.starts_with(STAGES[*stage]))
        .unwrap_or(0);
    (stage as u8, waxed)
}

/// `CopperChestBlock.getFromCopperBlock(copper, facing, …)`: the copper chest of the
/// block's age and wax, facing as the pumpkin did, single (named in the module's
/// comment).
pub(crate) fn chest_of(copper: BlockState, facing: &str) -> BlockState {
    let (stage, waxed) = copper_age(copper);
    let name = format!(
        "minecraft:{}{}copper_chest",
        if waxed { "waxed_" } else { "" },
        STAGES[usize::from(stage)]
    );
    let chest = clustine_data::block_id_by_name(&name)
        .map_or(blocks::COPPER_CHEST, |id| BLOCKS[id].default_state);
    set(chest, "facing", facing)
}

impl Region {
    /// `CarvedPumpkinBlock.onPlace`'s copper golem (`trySpawnGolem`, after the snow and
    /// iron golems' patterns, which N7k brings): a pumpkin placed, not changed from
    /// itself, on a block of `#copper` clears both with their breaking shown, makes an
    /// unwaxed golem of the copper's age where the pumpkin was (`TRIGGERED`: no
    /// `finalizeSpawn`), tells the neighbours of both, puts the copper chest where the
    /// copper was, and plays the golem's spawn sound.
    pub(super) fn carved_pumpkin_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_pumpkin(state) || same_block(old, state) {
            return;
        }
        let below = pos.offset(0, -1, 0);
        let Some(copper) = self.block(below) else {
            return;
        };
        if !in_tag(copper, "minecraft:copper")
            || !self.chunks.contains_key(&pos.chunk())
            || !self.chunks.contains_key(&below.chunk())
        {
            return;
        }
        // `clearPatternBlocks`: the pattern's blocks in its order, the pumpkin first.
        for (at, was) in [(pos, state), (below, copper)] {
            self.set_block_updated(at, blocks::AIR, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            output.events.push(RegionEvent::LevelEvent {
                kind: 2001,
                position: at,
                data: i32::from(was.0),
            });
        }
        let (weather, _) = copper_age(copper);
        let mut mob = Mob::fresh();
        copper_golem::update(&mut mob, |golem| golem.weather = weather);
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.05,
            f64::from(pos.z) + 0.5,
        ];
        let kind = entity_types::COPPER_GOLEM as u16;
        let made = self.new_mob(kind, mob, at, 0.0, 0.0);
        if let Some(golem) = made {
            self.add_made_mob(golem, output);
        }
        // `updatePatternBlocks`.
        self.update_neighbours_at(pos, blocks::AIR, output);
        self.update_neighbours_at(below, blocks::AIR, output);
        // `replaceCopperBlockWithChest`, of the states the pattern matched.
        let facing = state.property("facing").unwrap_or("north");
        let chest = chest_of(copper, facing);
        self.set_block_updated(below, chest, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        // `spawn(weatherState)`'s `playSpawnSound`.
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_COPPER_GOLEM_SPAWN,
            source: crate::physics::item::SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(at[0], at[1], at[2]),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// The copper golems of the region's own land that have the chest at `pos` open
    /// (`hasContainerOpen`) and whose boxes meet the block's grown by their reach of 3
    /// and four more, as `ContainerOpenersCounter.getEntitiesWithContainerOpen` finds
    /// them.
    pub(super) fn golems_with_chest_open(&self, pos: BlockPos) -> u32 {
        let reach = CONTAINER_INTERACTION_RANGE + 4.0;
        let area = Aabb::of_block(pos).inflate([reach; 3]);
        let info = EntityTypeInfo::of(entity_types::COPPER_GOLEM);
        let centre = pos.chunk();
        let mut found = 0_u32;
        for dx in -1..=1 {
            for dz in -1..=1 {
                let chunk = clustine_world::ChunkPos::new(centre.x + dx, centre.z + dz);
                let Some(held) = self.chunks.get(&chunk) else {
                    continue;
                };
                for entity in held.entities() {
                    if i32::from(entity.kind) != entity_types::COPPER_GOLEM {
                        continue;
                    }
                    let Ok(mob) = Mob::decode(&entity.data) else {
                        continue;
                    };
                    let own = copper_golem::state_of(&mob);
                    let open = copper_golem::has_container_open(&own, pos, |opened| {
                        let state = self.block(opened)?;
                        let paired = is_class(state, BlockClass::ChestBlock)
                            && matches!(state.property("type"), Some("left" | "right"));
                        paired
                            .then(|| chest_connected_direction(state))
                            .flatten()
                            .map(|towards| relative(opened, towards))
                    });
                    let meets =
                        Aabb::of_entity(entity.position, info.width, info.height).intersects(area);
                    if open && meets {
                        found = found.saturating_add(1);
                    }
                }
            }
        }
        found
    }

    /// `CopperGolemStatueBlock.useItemOn` with anything but an axe (`updatePose`): its
    /// sound at the block, the statue in its next pose with every update, and
    /// `BLOCK_CHANGE` of the player.
    pub(super) fn statue_used(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_COPPER_GOLEM_BECOME_STATUE,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
        self.set_block(pos, copper_golem::next_pose(state), output);
        let context = Context::of(self.player_source(player), None);
        self.vibrate_at(Vibration::BLOCK_CHANGE, pos, context);
    }
}
