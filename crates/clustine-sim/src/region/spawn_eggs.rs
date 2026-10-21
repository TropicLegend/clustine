//! Spawn eggs (`SpawnEggItem`, `EntityType.spawn` and `create`, `Mob.finalizeSpawn`;
//! `docs/groundwork/n7-brief.md`, section 1.11): an egg used on a block makes a mob of
//! its type in the block, where that has no shape, or beside it, standing on what is
//! there; one used where the player looks at water or lava makes it at the bottom of
//! the source block met; one used on a spawner sets what the spawner spawns.
//!
//! Every spawn egg is one class read from data: its type is its item's default
//! `entity_data` (`ItemComponents::entity`). A mob is made with its body's defaults:
//! its type's attributes, its health at their most, its follow range's bonus drawn on a
//! triangle and its hand, then its ambient sound. Its own draws (its UUID, the yaw its
//! constructor draws and the spawn turns over) come from its key's stream; the level's
//! on its behalf (its yaw, the follow range's two, the hand's) from the player's stream
//! aside, in that order (`docs/groundwork/n7-plan.md`, section 7.2.8).
//!
//! An egg used on a mob of its own type makes a baby of it (`region/animals.rs`).
//!
//! Named differences: a spawner set by an egg spawns nothing until N9; a trial spawner
//! is not set; an egg in a dispenser is not used (`dispensers/behaviours.rs`); a renamed
//! egg names nothing; a creative player keeps the egg, and every player is one.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, DEFAULT_ATTRIBUTES, EntityTypeInfo, ItemComponents, entity_types};
use clustine_data::{attributes as ids, blocks};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityId, PlayerId};

use super::Region;
use super::block_entities::{nbt_put_string, nbt_string, skip_nbt};
use super::entities::insecure_uuid;
use super::items::View;
use super::riding::wrap_degrees;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{Face, RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::body::attributes::{ADD_MULTIPLIED_BASE, RANDOM_SPAWN_BONUS, supplied};
use crate::mobs::body::hurt::voice_pitch;
use crate::mobs::body::{Head, Modifier, attribute};
use crate::mobs::types;
use crate::physics::aabb::Aabb;
use crate::physics::collide;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::placement::{Click, ItemUse};
use crate::region::Random;

const TAG_COMPOUND: u8 = 10;
const TAG_STRING: u8 = 8;
const TAG_END: u8 = 0;

/// The mob type the egg `item` makes, if it is a spawn egg of a living type: every
/// mob's but a player's and an armour stand's.
fn egg_type(item: i32) -> Option<u16> {
    let kind = ItemComponents::of(item).entity?;
    let id = i32::from(kind);
    let living = DEFAULT_ATTRIBUTES
        .get(usize::from(kind))
        .is_some_and(Option::is_some);
    (living && id != entity_types::PLAYER && id != entity_types::ARMOR_STAND).then_some(kind)
}

/// The block one step from `pos` across `face`.
fn beside(pos: BlockPos, face: Face) -> BlockPos {
    match face {
        Face::Bottom => pos.offset(0, -1, 0),
        Face::Top => pos.offset(0, 1, 0),
        Face::North => pos.offset(0, 0, -1),
        Face::South => pos.offset(0, 0, 1),
        Face::West => pos.offset(-1, 0, 0),
        Face::East => pos.offset(1, 0, 0),
    }
}

/// `Mob.finalizeSpawn`'s own part, after its type's: the follow range's bonus
/// (`random_spawn_bonus`, a triangle of ±0.11485 about 0 on the base, two `nextDouble`)
/// where it has none, and one hand in twenty the left (`nextFloat() < 0.05f`), drawn
/// from `level`.
pub(crate) fn finalize_spawn(kind: u16, mob: &mut Mob, level: &mut Random) {
    let follow = attribute(ids::FOLLOW_RANGE);
    if let Some(instance) = mob.body.attributes.instance(kind, follow)
        && !instance.has_modifier(RANDOM_SPAWN_BONUS)
    {
        // `RandomSource.triangle(mode, deviation)`.
        let amount = 0.0 + 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        instance.add_modifier(Modifier {
            id: RANDOM_SPAWN_BONUS.to_owned(),
            amount,
            operation: ADD_MULTIPLIED_BASE,
        });
    }
    mob.body.flags.left_handed = level.next_float() < 0.05;
}

impl Region {
    /// `SpawnEggItem.useOn` of the click `click` by the region's player `player`, whose
    /// clicked block the region holds: on a spawner, it sets what the spawner spawns;
    /// elsewhere a mob of the egg's type in the clicked block where it has no shape,
    /// else beside it, put on what is under it (looking a block lower on a block's top).
    /// Whether it did anything.
    pub(crate) fn use_spawn_egg_on(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(kind) = click.held.and_then(egg_type) else {
            return false;
        };
        let pos = click.position;
        let Some(state) = self.block(pos) else {
            return false;
        };
        if is_class(state, BlockClass::SpawnerBlock) {
            return self.set_spawner_entity(pos, kind, player, output);
        }
        if is_class(state, BlockClass::TrialSpawnerBlock) {
            return false;
        }
        let spawn_at = if state.voxel_collision_shape().is_empty() {
            pos
        } else {
            beside(pos, click.face)
        };
        let deeper = spawn_at != pos && click.face == Face::Top;
        if !crate::mobs::types::is_built(kind) {
            return false;
        }
        self.spawn_mob(kind, player, spawn_at, true, deeper, output)
            .is_some()
    }

    /// `SpawnEggItem.use` of the region's player `player`'s egg where they look: a
    /// source of water or lava met by their ray within reach, the mob made at the
    /// bottom of its block. Whether it made one.
    pub(crate) fn use_spawn_egg(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(kind) = used.held.and_then(egg_type) else {
            return false;
        };
        let Ok(Some(pos)) = crate::placement::source_hit(used, &super::Readable(self)) else {
            return false;
        };
        let Some(state) = self.block(pos) else {
            return false;
        };
        // A `LiquidBlock`: water or lava itself, not a block waterlogged.
        let liquid = clustine_blocks::data::is_block(state, blocks::WATER)
            || clustine_blocks::data::is_block(state, blocks::LAVA);
        if !liquid || !crate::mobs::types::is_built(kind) {
            return false;
        }
        self.spawn_mob(kind, player, pos, false, false, output)
            .is_some()
    }

    /// `EntityType.spawn(level, stack, player, pos, SPAWN_ITEM_USE, offset, deeper)` of
    /// a mob of the type `kind` at the block `pos`: at its middle, put on what collides
    /// in it and below where `offset`, turned at random, with its body's defaults and
    /// its type's `finalizeSpawn`; added, or sent at once where its chunk is not the
    /// region's to tick; then its ambient sound. Its key, where it made one.
    pub(crate) fn spawn_mob(
        &mut self,
        kind: u16,
        player: PlayerId,
        pos: BlockPos,
        offset: bool,
        deeper: bool,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        self.spawn_mob_with(kind, player, pos, (offset, deeper), false, |_| {}, output)
    }

    /// [`Region::spawn_mob`], `offset` and `deeper` as a pair, with `made` done to the
    /// mob after its `finalizeSpawn` and before it is added (a bucket's
    /// `setFromBucket(true)`); `poured` where a bucket pours it (`BUCKET`), whose
    /// `finalizeSpawn` a type may skip whole.
    #[expect(
        clippy::too_many_arguments,
        reason = "`EntityType.create`'s arguments, and what its caller does after"
    )]
    pub(crate) fn spawn_mob_with(
        &mut self,
        kind: u16,
        player: PlayerId,
        pos: BlockPos,
        (offset, deeper): (bool, bool),
        poured: bool,
        made: impl FnOnce(&mut Mob),
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let chunk = pos.chunk();
        if !self.readable(chunk) {
            return None;
        }
        let info = EntityTypeInfo::of(i32::from(kind));
        let (x, z) = (f64::from(pos.x) + 0.5, f64::from(pos.z) + 0.5);
        let y = if offset {
            let raised = Aabb::of_entity([x, f64::from(pos.y) + 1.0, z], info.width, info.height);
            f64::from(pos.y) + self.spawn_y_offset(pos, deeper, &raised)
        } else {
            f64::from(pos.y)
        };
        let player_key = self.players.get(&player)?.entity_id;
        let here = self.ticks_chunk(ChunkPos::containing(x, z));
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        // The new mob's own draws: `Entity`'s UUID, `LivingEntity`'s yaw, turned over.
        let mut own = self.entity_stream(key).random;
        let own_start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _constructor_yaw = own.next_float();
        // The type's constructor's own draws (a chicken's egg timer).
        let mob_type = types::of(kind);
        let mut mob = Mob::fresh();
        mob_type.construct(&mut mob, &mut own);
        // The level's draws on its behalf, from the player's stream aside.
        let mut level = self.entity_stream(player_key).random;
        let level_start = level.clone();
        let yaw = wrap_degrees(level.next_float() * 360.0);
        // `setHealth(getMaxHealth())` in the constructor, of its type's attributes.
        mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap_or(20.0) as f32;
        mob.body.vitals.air = mob_type.max_air();
        mob.body.head = Head {
            head_yaw: yaw,
            body_yaw: yaw,
            still_since: 0,
            ..Head::default()
        };
        // `finalizeSpawn`: the type's, with the biome at its block, then `Mob`'s.
        let block = crate::physics::containing([x, y, z]);
        let biome = self.biome_at(block);
        let day = self.clock.day;
        if !poured || mob_type.finalizes_when_poured() {
            mob_type.finalize_spawn(&mut types::Spawning {
                mob: &mut mob,
                level: &mut level,
                own: &mut own,
                biome,
                at: block,
                day,
            });
            finalize_spawn(kind, &mut mob, &mut level);
            mob_type.after_finalize_spawn(&mut types::Spawning {
                mob: &mut mob,
                level: &mut level,
                own: &mut own,
                biome,
                at: block,
                day,
            });
        }
        made(&mut mob);
        self.entity_drew(player_key, level.draws_since(&level_start));
        let entity = Entity {
            key,
            kind,
            uuid,
            position: [x, y, z],
            motion: [0.0; 3],
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: mob.encode(),
        };
        let at = entity.position;
        // `playAmbientSound()`: the voice's pitch drawn of its own random where its
        // type has a sound.
        let now = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let sound = mob_type
            .ambient_sound_drawn(&mob, &mut own, now, 0, None)
            .map(|sound| {
                let pitch = voice_pitch(&mut own, mob_type.is_baby(&mob));
                (sound, pitch)
            });
        // The type's own sound of its `finalizeSpawn` (a copper golem's), played before
        // the mob is added and its ambient sound.
        let spawned = mob_type.spawn_sound(&mob);
        if here {
            self.entity_drew(key, own.draws_since(&own_start));
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        if let Some(sound) = spawned {
            output.events.push(RegionEvent::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        if let Some((sound, pitch)) = sound {
            output.events.push(RegionEvent::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(at[0], at[1], at[2]),
                volume: mob_type.sound_volume(),
                pitch,
                except: None,
            });
        }
        // `gameEvent(player, ENTITY_PLACE, pos)` is the caller's (`placing.rs`).
        Some(key)
    }

    /// `EntityType.getYOffset(level, pos, deeper, box)`: how far above `pos` the box
    /// `raised` (the mob's, a block up) comes to rest moving down one, or two where it
    /// looks a block lower, on the shapes of the block (and the one below).
    fn spawn_y_offset(&self, pos: BlockPos, deeper: bool, raised: &Aabb) -> f64 {
        let view = View {
            region: self,
            home: pos.chunk(),
        };
        let mut area = Aabb::of_block(pos);
        if deeper {
            area = area.expand_towards([0.0, -1.0, 0.0]);
        }
        let shapes: Vec<_> = collide::block_collisions(&view, NO_ENTITY, &area)
            .into_iter()
            .map(|found| found.shape)
            .collect();
        let down = if deeper { -2.0 } else { -1.0 };
        1.0 + collide::collide_with_shapes([0.0, down, 0.0], raised, &shapes)[1]
    }

    /// `SpawnEggItem.useOn` on a spawner: `BaseSpawner.setEntityId(type)`, the type
    /// named in its next spawn's entity, the rest of its data as it was; clients told
    /// of the block entity; sculk hears the block change.
    fn set_spawner_entity(
        &mut self,
        pos: BlockPos,
        kind: u16,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(data) = self.own_block_entity_data(pos).map(<[u8]>::to_vec) else {
            return false;
        };
        let Some(name) = clustine_data::ENTITY_TYPES.get(usize::from(kind)) else {
            return false;
        };
        let Some(changed) = with_spawn_entity(&data, name) else {
            return false;
        };
        if !self.set_block_entity_data(pos, changed, output) {
            return false;
        }
        let source = self
            .players
            .get(&player)
            .map(|record| Source::of(record.entity_id));
        if let Some(vibration) = Vibration::named("minecraft:block_change") {
            self.vibrate_at(vibration, pos, Context::of(source, self.block(pos)));
        }
        true
    }
}

/// `CollisionContext.empty()`: what a block's shape is to no entity at all.
const NO_ENTITY: collide::Context = collide::Context {
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// A spawner's data (one nameless compound, its type byte first) with its next spawn's
/// entity of the type `id`: the `id` of `SpawnData`'s `entity` set, everything else as
/// it was, the missing compounds made. `None` for data that is not a compound.
pub(crate) fn with_spawn_entity(data: &[u8], id: &str) -> Option<Vec<u8>> {
    let (&tag, body) = data.split_first()?;
    if tag != TAG_COMPOUND {
        return None;
    }
    let mut out = vec![TAG_COMPOUND];
    out.extend(set_in_compound(body, &["SpawnData", "entity"], id)?);
    Some(out)
}

/// The entries of the compound `body` (up to and with its end), with the string `id`
/// at the path `path` of compounds below it set to `value`.
fn set_in_compound(body: &[u8], path: &[&str], value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut rest = body;
    let mut found = false;
    loop {
        let (&tag, after) = rest.split_first()?;
        if tag == TAG_END {
            break;
        }
        let (name, payload) = nbt_string(after)?;
        let next = skip_nbt(tag, payload, 1)?;
        let whole = &rest[..rest.len() - next.len()];
        match path.first() {
            Some(&wanted) if tag == TAG_COMPOUND && name == wanted.as_bytes() => {
                found = true;
                out.push(TAG_COMPOUND);
                nbt_put_string(&mut out, wanted);
                out.extend(set_in_compound(payload, &path[1..], value)?);
            }
            None if tag == TAG_STRING && name == b"id" => {
                found = true;
                out.push(TAG_STRING);
                nbt_put_string(&mut out, "id");
                nbt_put_string(&mut out, value);
            }
            _ => out.extend_from_slice(whole),
        }
        rest = next;
    }
    if !found {
        match path.first() {
            Some(&wanted) => {
                out.push(TAG_COMPOUND);
                nbt_put_string(&mut out, wanted);
                out.extend(set_in_compound(&[TAG_END], &path[1..], value)?);
            }
            None => {
                out.push(TAG_STRING);
                nbt_put_string(&mut out, "id");
                nbt_put_string(&mut out, value);
            }
        }
    }
    out.push(TAG_END);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The NBT of a named tag of type `tag`: its type, its name, then `payload`.
    fn named(tag: u8, name: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        nbt_put_string(&mut out, name);
        out.extend_from_slice(payload);
        out
    }

    fn string(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        nbt_put_string(&mut out, text);
        out
    }

    /// `entries` as one nameless compound, its type byte first.
    fn compound(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![TAG_COMPOUND];
        out.extend(body(entries));
        out
    }

    fn body(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out: Vec<u8> = entries.concat();
        out.push(TAG_END);
        out
    }

    #[test]
    fn an_egg_names_its_type_in_a_spawner_s_next_spawn_and_keeps_the_rest_of_its_data() {
        let delay = named(2, "Delay", &20_i16.to_be_bytes());
        let of = |id: &str| {
            let entity = named(
                TAG_COMPOUND,
                "entity",
                &body(&[named(TAG_STRING, "id", &string(id))]),
            );
            named(TAG_COMPOUND, "SpawnData", &body(&[entity]))
        };
        let zombie = compound(&[delay.clone(), of("minecraft:zombie")]);
        assert_eq!(
            with_spawn_entity(&zombie, "minecraft:pig"),
            Some(compound(&[delay.clone(), of("minecraft:pig")]))
        );
        // A spawner with no next spawn yet gets one of the type.
        let bare = compound(std::slice::from_ref(&delay));
        assert_eq!(
            with_spawn_entity(&bare, "minecraft:pig"),
            Some(compound(&[delay, of("minecraft:pig")]))
        );
        assert_eq!(with_spawn_entity(&[8, 0], "minecraft:pig"), None);
    }
}
