//! What golems do to the world about them (`docs/groundwork/n7k-brief.md`, sections 1.4,
//! 1.5 and 11.7; the plan's sections 2.6 and 10): a carved pumpkin or a jack o'lantern
//! placed builds the first golem whose pattern it completes, in the game's order and
//! in any of the pattern's orientations (`CarvedPumpkinBlock.trySpawnGolem`,
//! [`super::golem_patterns`]); a snow golem's trail; and an iron golem's poppy.
//!
//! At a line: a golem is built only where every cell of the first match is the
//! region's own land; where one is not, nothing is built and no later match is tried,
//! since clearing cells beyond the line would need every effect to land before the
//! golem stands (the plan's section 10). A snow golem's trail beyond the line is an
//! effect that holds only where the cell is still air, without its game event; a poppy
//! is given only to a mob of the region's own.
//!
//! Named differences: the advancement for building one (`SUMMONED_ENTITY` of the
//! players within 5) is not kept; a dispenser does not place a pumpkin
//! (`canSpawnGolem`, the dispenser's question, is not asked).

use clustine_blocks::data::{entity_in_tag, is_block, same_block};
use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::Region;
use super::copper_golems::{chest_of, copper_age};
use super::golem_patterns::{self, Golem, Match};
use super::updates::{UPDATE_ALL, UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::types::{copper_golem, iron_golem};

/// `PUMPKINS_PREDICATE`: a carved pumpkin or a jack o'lantern.
fn is_pumpkin(state: BlockState) -> bool {
    is_block(state, blocks::CARVED_PUMPKIN) || is_block(state, blocks::JACK_O_LANTERN)
}

/// The middle of the block at `pos` (`Vec3.atCenterOf`).
fn centre(pos: BlockPos) -> [f64; 3] {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

impl Region {
    /// `CarvedPumpkinBlock.onPlace`: a pumpkin placed, not changed from itself, asks
    /// `trySpawnGolem`: the snow golem's pattern, then the iron golem's, then the copper
    /// golem's, the first found built where all its cells are the region's own.
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
        let Some((golem, found)) = golem_patterns::first_golem(pos, |at| self.block(at)) else {
            return;
        };
        let cells = golem_patterns::pattern_cells(&found);
        if !cells.iter().all(|cell| self.own_block(*cell).is_some()) {
            return;
        }
        self.build_golem(golem, &found, &cells, output);
    }

    /// `trySpawnGolem`'s building of `golem` from `found`: its cells cleared, each with
    /// its breaking shown (`clearPatternBlocks`); the golem made with no `finalizeSpawn`
    /// (`TRIGGERED`), an iron golem marked as a player's, standing at its cell facing
    /// south; its neighbours told (`updatePatternBlocks`); and for a copper golem, its
    /// chest where the copper was and its spawn sound.
    fn build_golem(
        &mut self,
        golem: Golem,
        found: &Match,
        cells: &[BlockPos],
        output: &mut TickOutput,
    ) {
        for x in 0..found.width {
            for y in 0..found.height {
                let at = found.block(x, y, 0);
                self.set_block_updated(at, blocks::AIR, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                // The state the search read; a cell it did not read (a space, which no
                // golem's full pattern has) is read now, and is air.
                let was = found
                    .state(x, y, 0)
                    .or_else(|| self.block(at))
                    .unwrap_or(blocks::AIR);
                output.events.push(RegionEvent::LevelEvent {
                    kind: 2001,
                    position: at,
                    data: i32::from(was.0),
                });
            }
        }
        let mut mob = Mob::fresh();
        let kind = match golem {
            Golem::Snow => entity_types::SNOW_GOLEM,
            Golem::Iron => {
                iron_golem::player_created(&mut mob);
                entity_types::IRON_GOLEM
            }
            Golem::Copper => {
                // `getWeatherStateFromPattern`: the age of the copper the search read.
                let copper = found.state(0, 1, 0).unwrap_or(blocks::COPPER_BLOCK);
                let (weather, _) = copper_age(copper);
                copper_golem::update(&mut mob, |golem| golem.weather = weather);
                entity_types::COPPER_GOLEM
            }
        };
        let at = golem.stand(found);
        if let Some(made) = self.new_mob(kind as u16, mob, at, 0.0, 0.0) {
            self.add_made_mob(made, output);
        }
        for at in cells {
            self.update_neighbours_at(*at, blocks::AIR, output);
        }
        if golem == Golem::Copper {
            // `replaceCopperBlockWithChest`: of the states the search read, the chest
            // facing as the pumpkin did.
            let copper_at = found.block(0, 1, 0);
            let copper = found.state(0, 1, 0).unwrap_or(blocks::COPPER_BLOCK);
            let facing = found
                .state(0, 0, 0)
                .and_then(|pumpkin| pumpkin.property("facing"))
                .unwrap_or("north");
            let chest = chest_of(copper, facing);
            self.set_block_updated(copper_at, chest, UPDATE_CLIENTS, UPDATE_LIMIT, output);
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
    }

    /// `setBlockAndUpdate(pos, state)` and `gameEvent(BLOCK_PLACE, pos, Context.of(mob,
    /// state))` of the mob `mob`'s tick ([`crate::mobs::tick::Local::PlaceBlock`]): in
    /// the region's own land the block with every update and its game event; in another
    /// region's, the effect that sets it only where it is still what the region read,
    /// and no game event (named).
    pub(super) fn mob_places_block(
        &mut self,
        mob: EntityId,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let own = self.own_block(pos).is_some();
        self.set_block_updated(pos, state, UPDATE_ALL, UPDATE_LIMIT, output);
        if own && let Some(vibration) = Vibration::named("minecraft:block_place") {
            let context = Context::of(Some(Source::of(mob)), Some(state));
            self.vibrate(vibration, centre(pos), context);
        }
    }

    /// `OfferFlowerGoal.stop`'s gift to the region's mob `key`
    /// ([`crate::mobs::tick::Local::GiveFlower`]): a poppy on the antenna of one of
    /// `#accepts_iron_golem_gift` that has nothing there, which it then always drops.
    pub(super) fn give_golem_flower(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((chunk, record)) = self
            .find_entity(key)
            .map(|(chunk, record)| (chunk, record.clone()))
        else {
            return;
        };
        if !entity_in_tag(i32::from(record.kind), "minecraft:accepts_iron_golem_gift") {
            return;
        }
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let antenna = usize::from(slots::SADDLE);
        if mob.body.equipment.slots[antenna]
            .clone()
            .is_some_and(|stack| stack.count > 0)
        {
            return;
        }
        mob.body.equipment.slots[antenna] = Some(ItemStack::new(items::POPPY, 1));
        crate::mobs::types::equine::guarantee_drop(&mut mob, slots::SADDLE);
        self.commit_mob(&record, chunk, &mob, output);
    }
}
