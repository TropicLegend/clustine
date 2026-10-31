//! The end crystal's item (`EndCrystalItem.useOn`, d EndCrystalItem.txt 10–121; family
//! E7 of `docs/groundwork/p-plan.md`, section 19.5 item 13): used on obsidian or bedrock
//! whose block above is air and whose 1 × 2 × 1 box above holds no entity, it places a
//! crystal at the bottom middle of that block, without its base, and in the End asks
//! the fight to try its respawn. A creative player keeps the item
//! (`ServerPlayerGameMode.useItemOn`), and every player is one.
//!
//! Named difference: players of other regions are not known here, so one standing in
//! the box across a line does not refuse the crystal (as for an armour stand,
//! `docs/groundwork/n5-plan.md`, section 7 item 3).

use clustine_blocks::data::is_block;
use clustine_data::{blocks, entity_types, items};
use clustine_world::{Dimension, EndCrystal, Entity, EntityData, PlayerId};

use super::Region;
use super::entities::insecure_uuid;
use super::vibrations::{Context, Source, Vibration};
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;
use crate::placement::Click;

/// The draws of a new crystal's own stream: its UUID's two, and `time`'s
/// `nextInt(100000)` in its constructor.
const CRYSTAL_DRAWS: u64 = 3;

impl Region {
    /// `EndCrystalItem.useOn` for the click `click`, whose clicked block the region holds,
    /// by the player `player`: whether a crystal was placed.
    pub(crate) fn place_end_crystal(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if click.held != Some(items::END_CRYSTAL) {
            return false;
        }
        let Some(clicked) = self.block(click.position) else {
            return false;
        };
        if !is_block(clicked, blocks::OBSIDIAN) && !is_block(clicked, blocks::BEDROCK) {
            return false;
        }
        // `level.isEmptyBlock(above)`.
        let above = click.position.offset(0, 1, 0);
        if !self
            .block(above)
            .is_some_and(|state| state.props().is_air())
        {
            return false;
        }
        let corner = [f64::from(above.x), f64::from(above.y), f64::from(above.z)];
        let bounds = Aabb::new(corner, [corner[0] + 1.0, corner[1] + 2.0, corner[2] + 1.0]);
        if !self.free_for_a_crystal(&bounds) {
            return false;
        }
        let chunk = above.chunk();
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };
        // `Entity.<init>`'s UUID, then the constructor's `time`, which nothing here
        // reads (`end_crystal.rs`).
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _time = own.next_int(100_000);
        let entity = Entity {
            portal: Default::default(),
            key,
            kind: u16::try_from(entity_types::END_CRYSTAL).expect("entity type ids fit 16 bits"),
            uuid,
            position: [corner[0] + 0.5, corner[1], corner[2] + 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            // `setShowBottom(false)`; no beam.
            data: EntityData::EndCrystal(EndCrystal {
                beam_target: None,
                show_bottom: false,
            }),
        };
        if here {
            self.entity_drew(key, CRYSTAL_DRAWS);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        // `gameEvent(player, ENTITY_PLACE, above)`, at the middle of the block above.
        let source = self
            .players
            .get(&player)
            .map(|own| Source::of(own.entity_id));
        self.vibrate_at(Vibration::ENTITY_PLACE, above, Context::of(source, None));
        // `level.getDragonFight() != null`: the End's.
        if self.config.dimension == Dimension::End {
            self.end_crystal_placed_in_the_end(output);
        }
        true
    }

    /// `level.getEntities(null, box).isEmpty()`: no entity of the region's chunks or its
    /// copies, nor any of its players, whose box meets `bounds`.
    fn free_for_a_crystal(&self, bounds: &Aabb) -> bool {
        if !self.colliders_in(bounds).is_empty() {
            return false;
        }
        !self.players.values().any(|player| {
            let at = player.pose.position;
            Aabb::of_entity([at.x, at.y, at.z], 0.6, 1.8).intersects(*bounds)
        })
    }
}
