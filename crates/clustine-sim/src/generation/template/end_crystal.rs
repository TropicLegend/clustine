//! An end crystal made with its chunk: each of the End's spikes has one on top
//! (`EndSpikeFeature.placeSpike`; `docs/groundwork/p-plan.md`, section 19.2, P10a). The
//! run keeps it as the game's proto chunk does, as its saved tag, and the chunk loads it
//! from that tag; the features' code hands that tag here as a template's entity.

use clustine_world::generation::Placed;
use clustine_world::{BlockPos, EndCrystal, Entity, EntityData};

use super::Tag;
use crate::region::{Random, insecure_uuid};

/// The crystal `tag` describes, put where `placed` says and keyed by its making: what
/// `EndCrystal.readAdditionalSaveData` reads (d-javap's EndCrystal.txt, offsets 0-36):
/// the beam's target where the tag has one, and `ShowBottom`, true where it is
/// missing. Its own stream gives its UUID as `Entity.<init>` draws it; the
/// constructor's draw of its time is the crystal's to keep when its record has one.
pub(super) fn end_crystal(world: u64, kind: u16, tag: &Tag, placed: Placed<'_>) -> Option<Entity> {
    let beam_target = match tag.get("beam_target").map(Tag::list) {
        Some([x, y, z]) => Some(BlockPos::new(
            x.number()? as i32,
            y.number()? as i32,
            z.number()? as i32,
        )),
        _ => None,
    };
    let show_bottom = tag
        .get("ShowBottom")
        .and_then(Tag::number)
        .is_none_or(|value| value != 0.0);
    let Placed {
        position,
        yaw,
        pitch,
        makings,
        ..
    } = placed;
    let key = makings.next()?.key()?;
    let mut own = Random::of_generated(world, key);
    let uuid = insecure_uuid(own.next_u64(), own.next_u64());
    Some(Entity {
        portal: Default::default(),
        key,
        kind,
        uuid,
        position,
        motion: [0.0; 3],
        yaw,
        pitch,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::EndCrystal(EndCrystal {
            beam_target,
            show_bottom,
        }),
    })
}

#[cfg(test)]
mod tests {
    use clustine_data::entity_types;
    use clustine_world::ChunkPos;
    use clustine_world::generation::Makings;

    use super::*;

    /// Puts a member's type and name, as NBT writes them.
    fn member(bytes: &mut Vec<u8>, kind: u8, name: &str) {
        bytes.push(kind);
        bytes.extend((name.len() as u16).to_be_bytes());
        bytes.extend(name.as_bytes());
    }

    /// A compound tag in NBT's nameless binary form: `id`, then `ShowBottom` where
    /// given, then `beam_target` where given.
    fn tag(show_bottom: Option<i8>, beam: Option<[i32; 3]>) -> Vec<u8> {
        let mut bytes = vec![10];
        member(&mut bytes, 8, "id");
        let id = "minecraft:end_crystal";
        bytes.extend((id.len() as u16).to_be_bytes());
        bytes.extend(id.as_bytes());
        if let Some(value) = show_bottom {
            member(&mut bytes, 1, "ShowBottom");
            bytes.push(value as u8);
        }
        if let Some(beam) = beam {
            member(&mut bytes, 11, "beam_target");
            bytes.extend(3_i32.to_be_bytes());
            for value in beam {
                bytes.extend(value.to_be_bytes());
            }
        }
        bytes.push(0);
        bytes
    }

    fn made(bytes: &[u8]) -> Vec<Entity> {
        let mut makings = Makings::new(2, ChunkPos::new(2, 0));
        let mut level = Random::of_entity(3, clustine_world::EntityId(5), 0);
        crate::generation::template_entity(
            11,
            bytes,
            false,
            Placed {
                position: [42.5, 80.0, 0.5],
                yaw: 123.5,
                pitch: 0.0,
                structure: None,
                biome: clustine_world::Biome(0),
                makings: &mut makings,
                level: &mut level,
            },
        )
    }

    #[test]
    fn a_spike_s_crystal_is_made_from_its_tag_where_it_stands_showing_its_base() {
        let made = made(&tag(Some(1), None));
        assert_eq!(made.len(), 1);
        let crystal = &made[0];
        assert_eq!(i32::from(crystal.kind), entity_types::END_CRYSTAL);
        assert_eq!(crystal.position, [42.5, 80.0, 0.5]);
        assert_eq!(crystal.yaw, 123.5);
        assert_eq!(
            crystal.data,
            EntityData::EndCrystal(EndCrystal {
                beam_target: None,
                show_bottom: true,
            })
        );
        // The same each time the chunk is made.
        assert_eq!(made, self::made(&tag(Some(1), None)));
    }

    #[test]
    fn a_crystal_s_tag_gives_its_beam_and_hides_its_base_where_it_says() {
        let made = made(&tag(Some(0), Some([0, 128, 0])));
        assert_eq!(
            made[0].data,
            EntityData::EndCrystal(EndCrystal {
                beam_target: Some(BlockPos::new(0, 128, 0)),
                show_bottom: false,
            })
        );
        // Without `ShowBottom` the base shows, as a new crystal's does.
        let made = self::made(&tag(None, None));
        assert_eq!(
            made[0].data,
            EntityData::EndCrystal(EndCrystal {
                beam_target: None,
                show_bottom: true,
            })
        );
    }
}
