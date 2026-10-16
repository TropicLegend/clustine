//! What a block entity's data becomes when the game loads it from a template and saves
//! it again (`BlockEntity.loadWithComponents`, then `saveWithoutMetadata`): each type
//! reads the members it knows, with its defaults where they are missing, and writes
//! them back in its own types; what it does not know is gone. Read from each class's
//! `loadAdditional` and `saveAdditional` in the game's jar.
//!
//! A type not listed here keeps the template's members as they are; the judge of
//! structures shows which types that is wrong for.

use clustine_protocol::nbt::Nbt;

/// One member a type loads and saves again.
#[derive(Debug, Clone, Copy)]
enum Member {
    /// Read as any number and written as an `int`, this one where it is missing.
    Int(&'static str, i32),
    /// Read as any number and written as a `float`, this one where it is missing.
    Float(&'static str, f32),
    /// Kept as it is where it is there, and written nowhere else.
    Kept(&'static str),
    /// Kept as it is, and written empty where it is missing (`ContainerHelper`'s
    /// `Items`, which is written even when the container is empty).
    List(&'static str),
    /// Kept as it is, and written as an empty compound where it is missing.
    Compound(&'static str),
}

use Member::{Compound, Float, Int, Kept, List};

/// What `BaseContainerBlockEntity` loads and saves: its lock and its name.
const CONTAINER: [Member; 2] = [Kept("lock"), Kept("CustomName")];

/// `AbstractFurnaceBlockEntity`, for the furnace, the blast furnace and the smoker.
const FURNACE: [Member; 7] = [
    Int("cooking_time_spent", 0),
    Int("cooking_total_time", 0),
    Int("lit_time_remaining", 0),
    Int("lit_total_time", 0),
    Float("speed_multiplier", 1.0),
    List("Items"),
    Compound("RecipesUsed"),
];

/// `BrewingStandBlockEntity`.
const BREWING_STAND: [Member; 6] = [
    Int("BrewTime", 0),
    Int("total_brew_time", 400),
    List("Items"),
    Int("Fuel", 0),
    Int("total_fuel", 20),
    Float("speed_multiplier", 1.0),
];

/// The containers that keep loot to be rolled (`RandomizableContainer`): with a loot
/// table they load no items and save none (`tryLoadLootTable`, `trySaveLootTable`).
const RANDOMIZABLE: [&str; 6] = [
    "minecraft:chest",
    "minecraft:trapped_chest",
    "minecraft:barrel",
    "minecraft:dispenser",
    "minecraft:dropper",
    "minecraft:hopper",
];

/// The members of a type, in the order its `saveAdditional` writes them, or `None`
/// for a type not listed here.
fn members(kind: &str) -> Option<Vec<Member>> {
    let with_container = |own: &[Member]| CONTAINER.iter().chain(own).copied().collect();
    Some(match kind {
        "minecraft:hopper" => with_container(&[List("Items"), Int("TransferCooldown", -1)]),
        kind if RANDOMIZABLE.contains(&kind) => with_container(&[List("Items")]),
        "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => {
            with_container(&FURNACE)
        }
        "minecraft:brewing_stand" => with_container(&BREWING_STAND),
        _ => return None,
    })
}

/// The template's members of a block entity of `kind` (a block entity type's name) as
/// the game saves them after loading them; `id`, `x`, `y`, `z`, a loot table and its
/// seed and `components` are the caller's.
pub fn resave(kind: &str, template: Vec<(String, Nbt)>) -> Vec<(String, Nbt)> {
    match kind {
        "minecraft:vault" => return vault(template),
        "minecraft:sign" | "minecraft:hanging_sign" => return sign(template),
        "minecraft:banner" => return banner(template),
        "minecraft:sculk_sensor" | "minecraft:calibrated_sculk_sensor" => {
            return sculk_sensor(template);
        }
        _ => {}
    }
    let Some(members) = members(kind) else {
        return template;
    };
    let has_table = template.iter().any(|(key, _)| key == "LootTable");
    let find = |key: &str| {
        template
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    let mut saved: Vec<(String, Nbt)> = Vec::new();
    for member in members {
        let (key, value) = match member {
            Int(key, default) => (
                key,
                Some(Nbt::Int(
                    find(key)
                        .and_then(|value| number(&value))
                        .map_or(default, |n| n as i32),
                )),
            ),
            Float(key, default) => (
                key,
                Some(Nbt::Float(
                    find(key)
                        .and_then(|value| number(&value))
                        .map_or(default, |n| n as f32),
                )),
            ),
            Kept(key) => (key, find(key)),
            // A container whose loot is still to be rolled keeps no items.
            List("Items") if has_table && RANDOMIZABLE.contains(&kind) => ("Items", None),
            List(key) => (key, Some(find(key).unwrap_or(Nbt::List(Vec::new())))),
            Compound(key) => (key, Some(find(key).unwrap_or(Nbt::Compound(Vec::new())))),
        };
        if let Some(value) = value {
            saved.push((key.to_owned(), value));
        }
    }
    // What the caller adds back is the caller's to keep.
    for key in ["LootTable", "LootTableSeed", "components"] {
        if let Some(value) = find(key) {
            saved.push((key.to_owned(), value));
        }
    }
    saved
}

/// `VaultBlockEntity`: its `config`, `server_data` and `shared_data` through their
/// codecs, which leave out what equals its default. The key item is an item stack,
/// which a codec never finds equal to its default, so it is always written.
fn vault(template: Vec<(String, Nbt)>) -> Vec<(String, Nbt)> {
    let find = |members: &[(String, Nbt)], key: &str| {
        members
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    let config = match find(&template, "config") {
        Some(Nbt::Compound(members)) => members,
        _ => Vec::new(),
    };
    let mut saved_config = Vec::new();
    if let Some(table) = find(&config, "loot_table")
        && table != Nbt::String("minecraft:chests/trial_chambers/reward".into())
    {
        saved_config.push(("loot_table".to_owned(), table));
    }
    for (key, default) in [("activation_range", 4.0), ("deactivation_range", 4.5)] {
        if let Some(value) = find(&config, key).and_then(|value| number(&value))
            && value != default
        {
            saved_config.push((key.to_owned(), Nbt::Double(value)));
        }
    }
    let key_item = find(&config, "key_item").unwrap_or_else(|| {
        Nbt::Compound(vec![
            ("count".to_owned(), Nbt::Int(1)),
            ("id".to_owned(), Nbt::String("minecraft:trial_key".into())),
        ])
    });
    saved_config.push(("key_item".to_owned(), key_item));
    if let Some(display) = find(&config, "override_loot_table_to_display") {
        saved_config.push(("override_loot_table_to_display".to_owned(), display));
    }
    let mut saved = vec![
        ("config".to_owned(), Nbt::Compound(saved_config)),
        ("server_data".to_owned(), Nbt::Compound(Vec::new())),
        ("shared_data".to_owned(), Nbt::Compound(Vec::new())),
    ];
    if let Some(components) = find(&template, "components") {
        saved.push(("components".to_owned(), components));
    }
    saved
}

/// `SignBlockEntity`: both sides through `SignText`'s codec, whose lines are text
/// components, and whether it is waxed. A component that is plain text with nothing
/// else is written as a bare string. What else the template has is kept.
fn sign(mut template: Vec<(String, Nbt)>) -> Vec<(String, Nbt)> {
    fn plain(line: Nbt) -> Nbt {
        match line {
            Nbt::Compound(members) if members.len() == 1 && members[0].0 == "text" => {
                match &members[0].1 {
                    Nbt::String(text) => Nbt::String(text.clone()),
                    _ => Nbt::Compound(members),
                }
            }
            other => other,
        }
    }
    for (key, value) in &mut template {
        if key != "front_text" && key != "back_text" {
            continue;
        }
        if let Nbt::Compound(side) = value {
            for (name, lines) in side.iter_mut() {
                if (name == "messages" || name == "filtered_messages")
                    && let Nbt::List(items) = lines
                {
                    let taken = std::mem::take(items);
                    *items = taken.into_iter().map(plain).collect();
                }
            }
        }
    }
    if !template.iter().any(|(key, _)| key == "is_waxed") {
        template.push(("is_waxed".to_owned(), Nbt::Byte(0)));
    }
    template
}

/// `BannerBlockEntity`: its patterns are written only where there are some.
fn banner(mut template: Vec<(String, Nbt)>) -> Vec<(String, Nbt)> {
    template.retain(|(key, value)| !(key == "patterns" && *value == Nbt::List(Vec::new())));
    template
}

/// `SculkSensorBlockEntity`: the last frequency it heard and its listener, the
/// listener's defaults where the template has none (no event, a selector that has
/// chosen nothing, no delay).
fn sculk_sensor(template: Vec<(String, Nbt)>) -> Vec<(String, Nbt)> {
    let find = |key: &str| {
        template
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    let frequency = find("last_vibration_frequency")
        .and_then(|value| number(&value))
        .map_or(0, |n| n as i32);
    let listener = find("listener").unwrap_or_else(|| {
        Nbt::Compound(vec![
            ("event_delay".to_owned(), Nbt::Int(0)),
            (
                "selector".to_owned(),
                Nbt::Compound(vec![("tick".to_owned(), Nbt::Long(-1))]),
            ),
        ])
    });
    let mut saved = vec![
        ("last_vibration_frequency".to_owned(), Nbt::Int(frequency)),
        ("listener".to_owned(), listener),
    ];
    if let Some(components) = find("components") {
        saved.push(("components".to_owned(), components));
    }
    saved
}

/// A numeric tag's value, as Java's `NumericTag` gives it to `getIntOr`.
fn number(value: &Nbt) -> Option<f64> {
    Some(match value {
        Nbt::Byte(n) => f64::from(*n),
        Nbt::Short(n) => f64::from(*n),
        Nbt::Int(n) => f64::from(*n),
        Nbt::Long(n) => *n as f64,
        Nbt::Float(n) => f64::from(*n),
        Nbt::Double(n) => *n,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_furnace_writes_its_timers_as_ints_and_its_defaults() {
        let template = vec![
            ("cooking_time_spent".to_owned(), Nbt::Short(0)),
            ("Items".to_owned(), Nbt::List(Vec::new())),
            ("BurnTime".to_owned(), Nbt::Short(0)),
        ];
        let saved = resave("minecraft:blast_furnace", template);
        assert_eq!(
            saved,
            vec![
                ("cooking_time_spent".to_owned(), Nbt::Int(0)),
                ("cooking_total_time".to_owned(), Nbt::Int(0)),
                ("lit_time_remaining".to_owned(), Nbt::Int(0)),
                ("lit_total_time".to_owned(), Nbt::Int(0)),
                ("speed_multiplier".to_owned(), Nbt::Float(1.0)),
                ("Items".to_owned(), Nbt::List(Vec::new())),
                ("RecipesUsed".to_owned(), Nbt::Compound(Vec::new())),
            ]
        );
    }

    #[test]
    fn a_brewing_stand_gains_its_totals_and_its_speed() {
        let template = vec![
            ("BrewTime".to_owned(), Nbt::Short(0)),
            ("Fuel".to_owned(), Nbt::Byte(0)),
        ];
        let saved = resave("minecraft:brewing_stand", template);
        assert!(saved.contains(&("total_brew_time".to_owned(), Nbt::Int(400))));
        assert!(saved.contains(&("total_fuel".to_owned(), Nbt::Int(20))));
        assert!(saved.contains(&("Fuel".to_owned(), Nbt::Int(0))));
        assert!(saved.contains(&("speed_multiplier".to_owned(), Nbt::Float(1.0))));
    }

    #[test]
    fn a_chest_with_a_loot_table_keeps_no_items_and_one_without_keeps_them() {
        let with_table = vec![
            ("Items".to_owned(), Nbt::List(Vec::new())),
            (
                "LootTable".to_owned(),
                Nbt::String("minecraft:chests/x".into()),
            ),
        ];
        let saved = resave("minecraft:barrel", with_table);
        assert!(!saved.iter().any(|(key, _)| key == "Items"));
        assert!(saved.iter().any(|(key, _)| key == "LootTable"));
        let saved = resave("minecraft:chest", Vec::new());
        assert_eq!(saved, vec![("Items".to_owned(), Nbt::List(Vec::new()))]);
    }

    #[test]
    fn a_vault_leaves_out_its_default_loot_table_and_keeps_its_key() {
        let config = Nbt::Compound(vec![
            (
                "loot_table".to_owned(),
                Nbt::String("minecraft:chests/trial_chambers/reward".into()),
            ),
            (
                "key_item".to_owned(),
                Nbt::Compound(vec![
                    ("count".to_owned(), Nbt::Int(1)),
                    ("id".to_owned(), Nbt::String("minecraft:trial_key".into())),
                ]),
            ),
        ]);
        let saved = resave("minecraft:vault", vec![("config".to_owned(), config)]);
        let Some((_, Nbt::Compound(config))) = saved.iter().find(|(key, _)| key == "config") else {
            panic!("a vault saves its config");
        };
        assert_eq!(config.len(), 1);
        assert_eq!(config[0].0, "key_item");
        assert!(saved.contains(&("server_data".to_owned(), Nbt::Compound(Vec::new()))));
        assert!(saved.contains(&("shared_data".to_owned(), Nbt::Compound(Vec::new()))));
    }

    #[test]
    fn a_signs_plain_lines_are_written_as_strings_and_it_is_not_waxed() {
        let line = || Nbt::Compound(vec![("text".to_owned(), Nbt::String(String::new()))]);
        let side = Nbt::Compound(vec![(
            "messages".to_owned(),
            Nbt::List(vec![line(), line(), line(), line()]),
        )]);
        let saved = resave("minecraft:sign", vec![("front_text".to_owned(), side)]);
        let Some((_, Nbt::Compound(front))) = saved.iter().find(|(key, _)| key == "front_text")
        else {
            panic!("the front is kept");
        };
        assert_eq!(front[0].1, Nbt::List(vec![Nbt::String(String::new()); 4]));
        assert!(saved.contains(&("is_waxed".to_owned(), Nbt::Byte(0))));
    }

    #[test]
    fn a_sculk_sensor_writes_its_frequency_and_its_listeners_defaults() {
        let saved = resave("minecraft:sculk_sensor", Vec::new());
        assert_eq!(
            saved[0],
            ("last_vibration_frequency".to_owned(), Nbt::Int(0))
        );
        assert_eq!(saved[1].0, "listener");
    }

    #[test]
    fn a_type_not_listed_keeps_the_templates_members() {
        let template = vec![("anything".to_owned(), Nbt::Short(3))];
        assert_eq!(resave("minecraft:bell", template.clone()), template);
    }
}
