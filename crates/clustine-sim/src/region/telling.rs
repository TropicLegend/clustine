//! What a mob is told of another entity by the rules of its kind, here or across a line
//! (`EntityUse::Told`; `docs/groundwork/p-plan.md`, sections 18.5 and 18.6): a piglin
//! angered at a player who opens a container or breaks a block piglins guard, or told
//! the target another of its kind took. The region delivers it, judging the mob's sight
//! of the entity where the telling asks it, and the mob's type takes it
//! (`MobType::told`), which families E5 and E6 of P12 fill for the piglins and hoglins.
//!
//! The scan for those to tell (`Region::anger_nearby_piglins`) is E5's to fill too.

use clustine_world::{Effect, Effected, EntityId, EntityUse, Told, ToldWhat};

use super::Region;
use crate::api::TickOutput;
use crate::physics::collide::{Context, collision_shape};
use crate::physics::{self, walk};

impl Region {
    /// Tells the mob `key` `told`: where the region holds it, its type takes it, and
    /// its record is committed if that changed it; where the region only copies its
    /// chunk, an effect for the region that holds it.
    pub(crate) fn tell_mob(&mut self, key: EntityId, told: &Told, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            if let Some(chunk) = self.copied_entity_chunk(key) {
                self.file_told(key, chunk, told);
            }
            return;
        };
        let _ = chunk;
        let seen = match told.what {
            ToldWhat::AngerAt { only_if_seen: true } => self.sees(&record, told.eyes),
            _ => true,
        };
        let tick = self.tick;
        self.struck_between_ticks(key, output, |struck| {
            struck.mob_type.told(struck.mob, told, seen, tick)
        });
    }

    /// The chunk of the copy that holds the entity `key`, where one does.
    fn copied_entity_chunk(&self, key: EntityId) -> Option<clustine_world::ChunkPos> {
        self.copies
            .iter()
            .find(|(_, chunk)| chunk.entities().iter().any(|entity| entity.key == key))
            .map(|(position, _)| *position)
    }

    /// The telling filed for the region that holds the copy's chunk.
    fn file_told(&mut self, key: EntityId, chunk: clustine_world::ChunkPos, told: &Told) {
        if !self.sends_to(chunk) {
            return;
        }
        let Some(entity) = self
            .copies
            .get(&chunk)
            .and_then(|copy| copy.entities().iter().find(|entity| entity.key == key))
        else {
            return;
        };
        let at = physics::containing(entity.position);
        self.made.push(Effect {
            at,
            what: Effected::OnEntity {
                key,
                by: EntityUse::Told(told.clone()),
            },
        });
    }

    /// `BehaviorUtils.canSee`'s `hasLineOfSight` of the mob `record` to `eyes`: from its
    /// eyes, within 128 blocks, with no block's collision shape in the way.
    fn sees(&self, record: &clustine_world::Entity, eyes: [clustine_world::Double; 3]) -> bool {
        let info = clustine_data::EntityTypeInfo::of(i32::from(record.kind));
        let from = [
            record.position[0],
            record.position[1] + f64::from(info.eye_height),
            record.position[2],
        ];
        let to = eyes.map(|value| value.0);
        if physics::distance_sqr(from, to).sqrt() > 128.0 {
            return false;
        }
        let context = Context {
            stands_on_lava: false,
            bottom: record.position[1],
            fall_distance: record.fall_distance,
            falling_block: false,
            noclip: None,
        };
        !walk::traverse(from, to, |pos| {
            let state = self.block(pos).unwrap_or(clustine_data::blocks::AIR);
            collision_shape(state, pos, context).is_some_and(|(shape, _)| {
                shape
                    .boxes()
                    .iter()
                    .any(|bounds| physics::body::clips_shape(bounds, from, to))
            })
        })
    }

    /// `PiglinAi.angerNearbyPiglins(level, player, only_if_seen)`: the idle piglins about
    /// the player (seen by them where `only_if_seen`) angered at them. Family E5 fills
    /// it; until then nobody is angered.
    pub(crate) fn anger_nearby_piglins(
        &mut self,
        player: clustine_world::PlayerId,
        only_if_seen: bool,
        output: &mut TickOutput,
    ) {
        let _ = (player, only_if_seen, output);
    }
}
