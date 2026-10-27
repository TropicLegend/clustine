//! `alertOthers` as the region applies it (`docs/groundwork/n9-plan.md`, section 8.1;
//! `mobs/ai/targets.rs`): the region's own mob alerted at once, one in a copy sent the
//! word, and its holder applying it by the same tests.

use clustine_data::entity_types;
use clustine_world::{Effected, Entity, EntityChange, EntityData, EntityId, EntityUse};

use super::thrown::{cow, lay, mob};
use super::*;
use crate::mobs::Mob;
use crate::mobs::tick::Local;
use crate::region::tnt::tests::{nothing, west_of_a_line, world};

const SEED: u64 = 0x00A1_E875;

/// The player who hurt the alerting zombie.
const ATTACKER: EntityId = EntityId(9_999);

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type id")
}

/// A mob of the type `id` at `feet` holding `target`, with no AI of its own.
fn monster(key: EntityId, id: i32, feet: [f64; 3], target: Option<EntityId>) -> Entity {
    let mut entity = cow(key, feet);
    entity.kind = kind(id);
    let mut own = Mob::decode(&entity.data).expect("a mob's record");
    own.body.combat.target = target;
    entity.data = own.encode();
    entity
}

/// The word a zombie hurt by the player sends.
fn zombie_alert() -> EntityUse {
    EntityUse::Alert {
        attacker: ATTACKER,
        kind: kind(entity_types::PLAYER),
        class: kind(entity_types::ZOMBIE),
    }
}

fn target_of(region: &Region, key: EntityId) -> Option<EntityId> {
    mob(region, key).body.combat.target
}

#[test]
fn an_alert_of_the_region_s_own_mob_is_applied_at_once_and_one_in_a_copy_is_sent_to_its_holder() {
    let mut region = west_of_a_line(SEED, nothing);
    let alerter = EntityId(500);
    let here = EntityId(501);
    lay(
        &mut region,
        &monster(here, entity_types::HUSK, [3.5, -60.0, 0.5], None),
    );
    let across = EntityId(77_777);
    let copied = monster(across, entity_types::DROWNED, [17.0, -60.0, 0.5], None);
    region
        .copies
        .get_mut(&copied.chunk())
        .expect("the column east of the line is copied")
        .apply_entity(&EntityChange::Whole(copied));
    let alert = |key| Local::Alert {
        key,
        attacker: ATTACKER,
        kind: kind(entity_types::PLAYER),
        class: kind(entity_types::ZOMBIE),
    };
    let mut output = TickOutput::default();
    region.apply_locals(alerter, vec![alert(here), alert(across)], &mut output);
    assert_eq!(target_of(&region, here), Some(ATTACKER));
    let sent: Vec<&EntityUse> = region
        .made
        .iter()
        .filter_map(|effect| match &effect.what {
            Effected::OnEntity { key, by } if *key == across => Some(by),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [&zombie_alert()]);
    assert!(
        !region
            .made
            .iter()
            .any(|effect| matches!(&effect.what, Effected::OnEntity { key, .. } if *key == here)),
        "the region's own is not sent the word"
    );
}

#[test]
fn the_holder_applies_an_alert_by_the_same_tests() {
    let mut holder = world(SEED, nothing);
    let drowned = EntityId(601);
    let busy = EntityId(602);
    let skeleton = EntityId(603);
    lay(
        &mut holder,
        &monster(drowned, entity_types::DROWNED, [3.5, -60.0, 0.5], None),
    );
    lay(
        &mut holder,
        &monster(
            busy,
            entity_types::ZOMBIE,
            [5.5, -60.0, 0.5],
            Some(EntityId(4)),
        ),
    );
    lay(
        &mut holder,
        &monster(skeleton, entity_types::SKELETON, [7.5, -60.0, 0.5], None),
    );
    let mut output = TickOutput::default();
    // A husk's alert is for husks alone.
    let husks = EntityUse::Alert {
        attacker: ATTACKER,
        kind: kind(entity_types::PLAYER),
        class: kind(entity_types::HUSK),
    };
    holder.entity_used(drowned, husks, &mut output);
    assert_eq!(target_of(&holder, drowned), None);
    // A zombie's comes to a drowned with no target, not to one with a target or a
    // skeleton.
    for key in [drowned, busy, skeleton] {
        holder.entity_used(key, zombie_alert(), &mut output);
    }
    assert_eq!(target_of(&holder, drowned), Some(ATTACKER));
    assert_eq!(target_of(&holder, busy), Some(EntityId(4)));
    assert_eq!(target_of(&holder, skeleton), None);
    // The target is committed with the record.
    assert!(output.entity_changes.iter().any(|(_, change)| matches!(
        change,
        EntityChange::Set(key, _) if *key == drowned
    )));
    let (_, record) = holder.find_entity(drowned).expect("here");
    assert!(matches!(record.data, EntityData::Mob { .. }));
}
