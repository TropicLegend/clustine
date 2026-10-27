//! What the copper golem does to blocks (`docs/groundwork/n7-brief.md`, sections 7.0.5
//! and 7.13.7; `docs/groundwork/n7j-plan.md`, section 3, item 9): a carved pumpkin or a
//! jack o'lantern placed beside a block of copper, in any of the pattern's orientations,
//! builds one where the pumpkin was, with a copper chest of the block's age where the
//! copper was (`region/golems.rs`, which builds every golem); a golem with a chest open counts
//! as one of its openers at the chest's recheck; what its sorting does to a chest after
//! its tick (`Local::ContainerTake` and `Local::ContainerPut`: the chest opened, closed,
//! and the stack moved between it and the golem's hand, both in one); and its statue
//! turns to its next pose when used.
//!
//! At a line: a golem is built only where the pumpkin and the copper are both the
//! region's own (`region/golems.rs`), counts only at the chests of its region's land, and works only at
//! those: a chest of a copy is neither opened nor written, and the golem that comes to
//! one takes it for unreachable as it closes it (the plan's section 3, item 9).
//!
//! Named differences: its chest stays single beside another copper chest
//! (`getChestType` is not ported); no advancement is given for building one.

use clustine_blocks::data::{is_class, relative, set};
use clustine_blocks::growth::chest_connected_direction;
use clustine_data::{BLOCKS, BlockClass, BlockState, EntityTypeInfo, blocks, entity_types, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, PlayerId};

use super::Region;
use super::vibrations::{Context, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::brain;
use crate::mobs::types::copper_golem::{self, CONTAINER_INTERACTION_RANGE, ChestStep, STAGES};
use crate::mobs::types::golem_transport;
use crate::physics::aabb::Aabb;

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

    /// A copper golem's step of its taking from the chest at `chest`
    /// (`Local::ContainerTake`).
    pub(super) fn golem_takes(
        &mut self,
        golem: EntityId,
        chest: BlockPos,
        step: ChestStep,
        output: &mut TickOutput,
    ) {
        self.golem_at_chest(golem, chest, step, true, output);
    }

    /// A copper golem's step of its putting into the chest at `chest`
    /// (`Local::ContainerPut`).
    pub(super) fn golem_puts(
        &mut self,
        golem: EntityId,
        chest: BlockPos,
        step: ChestStep,
        output: &mut TickOutput,
    ) {
        self.golem_at_chest(golem, chest, step, false, output);
    }

    /// The golem `golem`'s step at the chest at `chest`, both halves of a double chest
    /// (`CompoundContainer`, the first half first), where the chest is all the region's
    /// own: opened as a player's menu opens it (the openers' count, the lid's event and
    /// sound, the recheck), closed likewise, or the stack moved. A chest of a copy is
    /// left as it is, and the golem takes it for unreachable as it closes it.
    fn golem_at_chest(
        &mut self,
        golem: EntityId,
        chest: BlockPos,
        step: ChestStep,
        takes: bool,
        output: &mut TickOutput,
    ) {
        let Some(halves) = self.chest_halves(chest) else {
            return;
        };
        let parts: Vec<BlockPos> = [Some(halves.first), halves.second]
            .into_iter()
            .flatten()
            .collect();
        if !parts.iter().all(|pos| self.own_block(*pos).is_some()) {
            if step == ChestStep::Close {
                self.golem_finds_unreachable(golem, chest, output);
            }
            return;
        }
        match step {
            ChestStep::Open => {
                for pos in parts {
                    self.start_open(pos, output);
                }
            }
            ChestStep::Close => {
                for pos in parts {
                    self.stop_open(pos, output);
                }
            }
            ChestStep::Move => self.golem_moves(golem, &parts, takes, output),
        }
    }

    /// `pickUpItems` or `putDownItem` of the golem `golem` at the chest of `parts`: up
    /// to 16 of the first stack into its empty hand, kept whatever kills it; or what it
    /// holds into the chest by the game's order, the rest left in its hand. The chest is
    /// written (and its comparators told) and the golem's hand shown, in one; nothing
    /// where the golem is no longer the region's or its hand is not as the move needs.
    fn golem_moves(
        &mut self,
        golem: EntityId,
        parts: &[BlockPos],
        takes: bool,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self
            .find_entity(golem)
            .map(|(chunk, record)| (chunk, record.clone()))
        else {
            return;
        };
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let mut containers = Vec::new();
        for pos in parts {
            let Some(mut container) = self.container_at(*pos) else {
                return;
            };
            container.unpack();
            containers.push((*pos, container));
        }
        let mut slots: Vec<Option<ItemStack>> = containers
            .iter()
            .flat_map(|(_, container)| container.slots.iter().cloned())
            .collect();
        let main_hand = usize::from(slots::MAIN_HAND);
        let hand = mob.body.equipment.slots[main_hand]
            .clone()
            .filter(|stack| stack.count > 0);
        let new_hand = if takes {
            if hand.is_some() {
                return;
            }
            let Some((index, count)) = golem_transport::pickup(&slots) else {
                return;
            };
            let Some(stack) = slots[index].as_mut() else {
                return;
            };
            stack.count -= count;
            let taken = stack.with_count(count);
            if stack.count <= 0 {
                slots[index] = None;
            }
            crate::mobs::types::equine::guarantee_drop(&mut mob, slots::MAIN_HAND);
            Some(taken)
        } else {
            let Some(held) = hand else {
                return;
            };
            golem_transport::add_items(&mut slots, held)
        };
        let mut rest = slots.into_iter();
        for (pos, mut container) in containers {
            let size = container.slots.len();
            container.slots = rest.by_ref().take(size).collect();
            self.set_container(pos, container, output);
        }
        mob.body.equipment.slots[main_hand] = new_hand;
        let fields = mob.changes_from(&record.data);
        if fields.is_empty() {
            return;
        }
        let mut after = record.clone();
        after.data = mob.encode();
        self.commit_entity(chunk, golem, fields, output);
        self.mob_shown_changes(&record, &after, chunk, output);
    }

    /// `markVisitedBlockPosAsUnreachable` of the golem `golem` for the chest at `chest`,
    /// which the region may not write: out of its visits and into its unreachable chests
    /// for 6000 ticks, so that its next searches pass it over.
    fn golem_finds_unreachable(
        &mut self,
        golem: EntityId,
        chest: BlockPos,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self
            .find_entity(golem)
            .map(|(chunk, record)| (chunk, record.clone()))
        else {
            return;
        };
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let provider = &copper_golem::PROVIDER;
        let mut memories = brain::memories_of(provider, &mob, record.lived);
        golem_transport::mark_unreachable_in(&mut memories, chest);
        brain::write_memories(provider, &mut mob, memories);
        let fields = mob.changes_from(&record.data);
        self.commit_entity(chunk, golem, fields, output);
    }
}
