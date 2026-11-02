//! The scene of a player's pose and appearance as another player is shown them, which
//! `poses.rs` runs on Clustine and `poses_oracle.rs` on both servers: a player changes
//! their skin's parts and main hand, sneaks while a third player comes to see them,
//! stands, glides with an elytra and lands.

use std::time::Duration;

use clustine_botswarm::Bot;
use clustine_data::{entity_types, items};
use clustine_protocol::packets::play::{EntityDataValue, pose};

const PATIENCE: Duration = Duration::from_secs(30);

/// The indices of a player's data the scene is about: the shared flags, the pose, the
/// main hand and the skin's parts. The rest (health, air and the like) the two servers
/// need not send alike for it.
const ABOUT: [u8; 4] = [0, 6, 15, 16];

/// The skin's parts the player changes to: the jacket, the right sleeve, the right
/// trouser leg and the hat (`PlayerModelPart` bits 0, 2, 4 and 6).
pub const SKIN_PARTS: u8 = 0x55;

/// What the others were shown of the player.
#[derive(Debug, PartialEq)]
pub struct Seen {
    /// Every `set_entity_data` of the player the watcher was sent, from the first, each
    /// with only the values of [`ABOUT`]; those left with none are left out.
    pub watched: Vec<Vec<(u8, EntityDataValue)>>,
    /// The values of [`ABOUT`] in the data the latecomer was first sent of the player,
    /// who sneaked as the latecomer came.
    pub paired: Vec<(u8, EntityDataValue)>,
}

/// Plays the scene on the server at `address`.
pub async fn run(address: &str) -> Seen {
    let mut player = Bot::join(address, "Poser").await.unwrap();
    player.wait_for_chunks(9, PATIENCE).await.unwrap();
    let mut watcher = Bot::join(address, "Watcher").await.unwrap();
    watcher.wait_for_chunks(9, PATIENCE).await.unwrap();
    let id = player.info.login.entity_id;
    watcher
        .wait_until(PATIENCE, |bot| sees(bot, id))
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher never saw the player"));

    // The options screen: other parts of the skin, and the left hand.
    player.set_appearance(SKIN_PARTS, 0).await.unwrap();
    watcher
        .wait_until(PATIENCE, |bot| {
            value_of(bot, id, 16) == Some(EntityDataValue::Byte(SKIN_PARTS))
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the skin's parts were not shown"));

    // Sneaking, while someone comes to see the player.
    player.sneak(true).await.unwrap();
    wait_for_pose(&mut watcher, id, pose::CROUCHING, "sneaking").await;
    let mut latecomer = Bot::join(address, "Latecomer").await.unwrap();
    latecomer
        .wait_until(PATIENCE, |bot| {
            sees(bot, id) && bot.player_data.iter().any(|(of, _)| *of == id)
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the latecomer was not shown the player"));
    let paired = latecomer
        .player_data
        .iter()
        .find(|(of, _)| *of == id)
        .map(|(_, values)| about(values))
        .unwrap_or_default();
    player.sneak(false).await.unwrap();
    wait_for_pose(&mut watcher, id, pose::STANDING, "standing up").await;

    // An elytra on, up in the air without flying, and a glide; then back on the ground,
    // which ends it.
    player.wear_on_chest(items::ELYTRA).await.unwrap();
    let (x, y, z) = player.location;
    let look = player.look;
    player.fly_to((x, y + 5.0, z), look, 0.5).await.unwrap();
    player.start_gliding().await.unwrap();
    wait_for_pose(&mut watcher, id, pose::FALL_FLYING, "gliding").await;
    player.stand_at(x, y, z).await.unwrap();
    wait_for_pose(&mut watcher, id, pose::STANDING, "landing").await;

    Seen {
        watched: watcher
            .player_data
            .iter()
            .filter(|(of, _)| *of == id)
            .map(|(_, values)| about(values))
            .filter(|values| !values.is_empty())
            .collect(),
        paired,
    }
}

/// What Clustine shows, as the game's rules have it.
pub fn expected() -> Seen {
    let flags = |flags| (0, EntityDataValue::Byte(flags));
    let pose = |pose| (6, EntityDataValue::Pose(pose));
    let left_hand = (
        15,
        EntityDataValue::Registered {
            serializer: 42,
            id: 0,
        },
    );
    let skin = |parts| (16, EntityDataValue::Byte(parts));
    Seen {
        watched: vec![
            // Coming into view: the bot's skin of the configuration, all seven parts.
            vec![skin(0x7F)],
            vec![left_hand.clone(), skin(SKIN_PARTS)],
            // The shift key's flag at once, the pose at the end of the player's tick.
            vec![flags(0x02)],
            vec![pose(pose::CROUCHING)],
            vec![flags(0)],
            vec![pose(pose::STANDING)],
            // The glide's flag at once, its pose at the end of the tick; on the ground
            // both end in the player's tick.
            vec![flags(0x80)],
            vec![pose(pose::FALL_FLYING)],
            vec![flags(0), pose(pose::STANDING)],
        ],
        paired: vec![
            flags(0x02),
            pose(pose::CROUCHING),
            left_hand,
            skin(SKIN_PARTS),
        ],
    }
}

fn sees(bot: &Bot, id: i32) -> bool {
    bot.entities
        .get(&id)
        .is_some_and(|entity| entity.kind == entity_types::PLAYER)
}

/// The latest value of `index` the bot was sent of the player `id`.
fn value_of(bot: &Bot, id: i32, index: u8) -> Option<EntityDataValue> {
    bot.player_data
        .iter()
        .filter(|(of, _)| *of == id)
        .flat_map(|(_, values)| values.iter())
        .filter(|(at, _)| *at == index)
        .map(|(_, value)| value.clone())
        .next_back()
}

async fn wait_for_pose(watcher: &mut Bot, id: i32, wanted: i32, what: &str) {
    watcher
        .wait_until(PATIENCE, |bot| {
            bot.entities.get(&id).and_then(|entity| entity.pose) == Some(wanted)
        })
        .await
        .unwrap_or_else(|error| panic!("{error}: the watcher was not shown {what}"));
}

fn about(values: &[(u8, EntityDataValue)]) -> Vec<(u8, EntityDataValue)> {
    values
        .iter()
        .filter(|(index, _)| ABOUT.contains(index))
        .cloned()
        .collect()
}
