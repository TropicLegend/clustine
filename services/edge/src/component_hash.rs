//! The game's hashes of item components (`docs/groundwork/n9-plan.md`, section 3.3): a
//! click in a menu carries each component of the stack the client thinks it holds as
//! `HashOps.CRC32C_INSTANCE`'s hash of the component's codec encoding, and the server
//! takes the client's guess as right only where its own stack's hashes are the same.
//!
//! `HashOps` hashes each value on its own with CRC-32C and builds a list or a map from
//! the hashes of what it holds: a tag byte, then the value's bytes little-endian (a
//! string its length in UTF-16 units and those units; a boolean one byte; a list the
//! hashes of its items in order; a map the hashes of its keys and values, entry by entry,
//! sorted by the key's hash and then the value's as unsigned numbers), then for a list or
//! a map a closing tag. The structure hashed is the one the containers' NBT is made from
//! (`clustine_world::components::structure`).

use clustine_world::Component;
use clustine_world::components::{Value, structure};

/// `HashOps`'s tags.
const TAG_EMPTY: u8 = 1;
const TAG_MAP_START: u8 = 2;
const TAG_MAP_END: u8 = 3;
const TAG_LIST_START: u8 = 4;
const TAG_LIST_END: u8 = 5;
const TAG_BYTE: u8 = 6;
const TAG_SHORT: u8 = 7;
const TAG_INT: u8 = 8;
const TAG_LONG: u8 = 9;
const TAG_FLOAT: u8 = 10;
const TAG_DOUBLE: u8 = 11;
const TAG_STRING: u8 = 12;
const TAG_BOOLEAN: u8 = 13;

/// The game's hash of `component`, as `HashedPatchMap` sends it: `HashOps.CRC32C_INSTANCE`
/// over the component's codec encoding, read as `HashCode.asInt()`.
#[allow(
    dead_code,
    reason = "the lead's join of `HashedStack` matching to the menus calls it"
)]
pub fn hash(component: &Component) -> i32 {
    hash_value(&structure(component)) as i32
}

/// `HashOps`'s hash of `value`, as the unsigned number `HashCode.asInt()` reads.
#[allow(
    dead_code,
    reason = "the lead's join of `HashedStack` matching to the menus calls it"
)]
fn hash_value(value: &Value) -> u32 {
    let mut bytes = Vec::new();
    match value {
        Value::Empty => bytes.push(TAG_EMPTY),
        Value::Byte(n) => {
            bytes.push(TAG_BYTE);
            bytes.extend(n.to_le_bytes());
        }
        Value::Short(n) => {
            bytes.push(TAG_SHORT);
            bytes.extend(n.to_le_bytes());
        }
        Value::Int(n) => {
            bytes.push(TAG_INT);
            bytes.extend(n.to_le_bytes());
        }
        Value::Long(n) => {
            bytes.push(TAG_LONG);
            bytes.extend(n.to_le_bytes());
        }
        // `putFloat` and `putDouble` write the raw bits, so a NaN keeps its own.
        Value::Float(n) => {
            bytes.push(TAG_FLOAT);
            bytes.extend(n.to_bits().to_le_bytes());
        }
        Value::Double(n) => {
            bytes.push(TAG_DOUBLE);
            bytes.extend(n.to_bits().to_le_bytes());
        }
        Value::Bool(b) => bytes.extend([TAG_BOOLEAN, u8::from(*b)]),
        Value::String(text) => {
            // `putInt(length())` and `putUnencodedChars`: Java's length is in UTF-16
            // units, each written as two bytes.
            let units: Vec<u16> = text.encode_utf16().collect();
            let length = i32::try_from(units.len()).expect("a string is shorter than 2^31 units");
            bytes.push(TAG_STRING);
            bytes.extend(length.to_le_bytes());
            for unit in units {
                bytes.extend(unit.to_le_bytes());
            }
        }
        Value::List(items) => {
            bytes.push(TAG_LIST_START);
            for item in items {
                bytes.extend(hash_value(item).to_le_bytes());
            }
            bytes.push(TAG_LIST_END);
        }
        Value::Map(entries) => {
            // `MAPLIKE_ENTRY_ORDER`: by the key's hash, then the value's, each compared
            // as `padToLong`, which is the unsigned number.
            let mut hashed: Vec<(u32, u32)> = entries
                .iter()
                .map(|(key, value)| (hash_value(&Value::String(key.clone())), hash_value(value)))
                .collect();
            hashed.sort_unstable();
            bytes.push(TAG_MAP_START);
            for (key, value) in hashed {
                bytes.extend(key.to_le_bytes());
                bytes.extend(value.to_le_bytes());
            }
            bytes.push(TAG_MAP_END);
        }
    }
    crc32c(&bytes)
}

/// CRC-32C (Castagnoli), as Guava's `Hashing.crc32c()` computes it: reflected, the
/// polynomial 0x1EDC6F41, all ones in and out.
fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc = CRC32C_TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8);
    }
    !crc
}

/// The remainders of each byte, for the reflected polynomial 0x82F63B78.
const CRC32C_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut at = 0;
    while at < 256 {
        let mut crc = at as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x82F6_3B78
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[at] = crc;
        at += 1;
    }
    table
};

#[cfg(test)]
mod tests {
    use clustine_world::components::{
        BannerLayer, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, Kind,
        PotionContents, Rarity, TooltipDisplay, UseCooldown, stack_structure,
    };
    use clustine_world::{Components, Float, ItemStack};

    use super::*;

    fn item(name: &str) -> i32 {
        clustine_data::item_id_by_name(name).expect("the item exists")
    }

    fn effect(name: &str) -> EffectInstance {
        EffectInstance {
            effect: name.to_owned(),
            amplifier: 0,
            duration: 0,
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }

    /// The game's NBT form of a structure as `NbtOps` and `Tag.toString` print it: keys
    /// sorted, quoted where they are not plain, booleans as bytes.
    fn snbt(value: &Value) -> String {
        let quoted =
            |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
        match value {
            Value::Empty => "END".to_owned(),
            Value::Byte(n) => format!("{n}b"),
            Value::Short(n) => format!("{n}s"),
            Value::Int(n) => format!("{n}"),
            Value::Long(n) => format!("{n}L"),
            Value::Float(n) => format!("{n:?}f"),
            Value::Double(n) => format!("{n:?}d"),
            Value::Bool(b) => format!("{}b", u8::from(*b)),
            Value::String(text) => quoted(text),
            Value::List(items) => {
                let items: Vec<String> = items.iter().map(snbt).collect();
                format!("[{}]", items.join(","))
            }
            Value::Map(entries) => {
                let mut entries: Vec<(&String, &Value)> =
                    entries.iter().map(|(key, value)| (key, value)).collect();
                entries.sort_by_key(|(key, _)| *key);
                let entries: Vec<String> = entries
                    .into_iter()
                    .map(|(key, value)| {
                        let plain = key
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c));
                        let key = if plain { key.clone() } else { quoted(key) };
                        format!("{key}:{}", snbt(value))
                    })
                    .collect();
                format!("{{{}}}", entries.join(","))
            }
        }
    }

    /// The game's `JsonOps` form, for comparing field for field; an int and a float stay
    /// apart, as `1` and `1.0` do in the JSON.
    fn as_json(value: &Value) -> serde_json::Value {
        use serde_json::Value as Json;
        match value {
            Value::Empty => Json::Null,
            Value::Byte(n) => Json::from(*n),
            Value::Short(n) => Json::from(*n),
            Value::Int(n) => Json::from(*n),
            Value::Long(n) => Json::from(*n),
            Value::Float(n) => Json::from(f64::from(*n)),
            Value::Double(n) => Json::from(*n),
            Value::Bool(b) => Json::Bool(*b),
            Value::String(text) => Json::String(text.clone()),
            Value::List(items) => Json::Array(items.iter().map(as_json).collect()),
            Value::Map(entries) => Json::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), as_json(value)))
                    .collect(),
            ),
        }
    }

    /// Each line of the game's answers (`HashAnswers.java`, run against the game's own
    /// classes with a registry context): what was hashed, its hash, its `JsonOps` form
    /// and its `NbtOps` form.
    fn answers() -> Vec<(&'static str, Value, i32, &'static str, &'static str)> {
        let component = |component: Component| structure(&component);
        vec![
            (
                "potion_contents swiftness",
                component(Component::PotionContents(PotionContents {
                    potion: Some("minecraft:swiftness".into()),
                    ..PotionContents::default()
                })),
                -1164789092,
                r#"{"potion":"minecraft:swiftness"}"#,
                r#"{potion:"minecraft:swiftness"}"#,
            ),
            (
                "potion_contents empty",
                component(Component::PotionContents(PotionContents::default())),
                -982207288,
                "{}",
                "{}",
            ),
            (
                "potion_contents water colour 0xFF0000 two effects named",
                component(Component::PotionContents(PotionContents {
                    potion: Some("minecraft:water".into()),
                    custom_colour: Some(0x00FF_0000),
                    custom_effects: vec![
                        EffectInstance {
                            amplifier: 1,
                            duration: 200,
                            ..effect("minecraft:speed")
                        },
                        EffectInstance {
                            duration: 100,
                            ambient: true,
                            visible: false,
                            show_icon: true,
                            ..effect("minecraft:poison")
                        },
                    ],
                    custom_name: Some("fancy".into()),
                })),
                538540122,
                r#"{"potion":"minecraft:water","custom_color":16711680,"custom_effects":[{"id":"minecraft:speed","amplifier":1,"duration":200,"show_icon":true},{"id":"minecraft:poison","duration":100,"ambient":true,"show_particles":false,"show_icon":true}],"custom_name":"fancy"}"#,
                r#"{custom_color:16711680,custom_effects:[{amplifier:1b,duration:200,id:"minecraft:speed",show_icon:1b},{ambient:1b,duration:100,id:"minecraft:poison",show_icon:1b,show_particles:0b}],custom_name:"fancy",potion:"minecraft:water"}"#,
            ),
            (
                "potion_duration_scale 0.25",
                component(Component::PotionDurationScale(Float(0.25))),
                -1754288493,
                "0.25",
                "0.25f",
            ),
            (
                "damage 5",
                component(Component::Damage(5)),
                645064431,
                "5",
                "5",
            ),
            (
                "damage 0",
                component(Component::Damage(0)),
                -2146075228,
                "0",
                "0",
            ),
            (
                "intangible_projectile",
                component(Component::IntangibleProjectile),
                -982207288,
                "{}",
                "{}",
            ),
            (
                "rarity epic",
                component(Component::Rarity(Rarity::Epic)),
                -292715907,
                r#""epic""#,
                r#""epic""#,
            ),
            (
                "rarity common",
                component(Component::Rarity(Rarity::Common)),
                75150990,
                r#""common""#,
                r#""common""#,
            ),
            (
                "item_name literal Hi",
                component(Component::ItemName(r#""Hi""#.into())),
                485089409,
                r#""Hi""#,
                r#""Hi""#,
            ),
            (
                "item_name translatable item.minecraft.apple",
                component(Component::ItemName(
                    r#"{"translate":"item.minecraft.apple"}"#.into(),
                )),
                -767870237,
                r#"{"translate":"item.minecraft.apple"}"#,
                r#"{translate:"item.minecraft.apple"}"#,
            ),
            (
                "use_cooldown 1",
                component(Component::UseCooldown(UseCooldown {
                    seconds: Float(1.0),
                    group: None,
                })),
                978556113,
                r#"{"seconds":1.0}"#,
                "{seconds:1.0f}",
            ),
            (
                "use_cooldown 2.5 group minecraft:ender_pearl",
                component(Component::UseCooldown(UseCooldown {
                    seconds: Float(2.5),
                    group: Some("minecraft:ender_pearl".into()),
                })),
                -1462084184,
                r#"{"seconds":2.5,"cooldown_group":"minecraft:ender_pearl"}"#,
                r#"{cooldown_group:"minecraft:ender_pearl",seconds:2.5f}"#,
            ),
            (
                "ominous_bottle_amplifier 2",
                component(Component::OminousBottleAmplifier(2)),
                1064459813,
                "2",
                "2",
            ),
            (
                "suspicious_stew_effects night_vision 100",
                component(Component::SuspiciousStewEffects(vec![(
                    "minecraft:night_vision".into(),
                    100,
                )])),
                -858533844,
                r#"[{"id":"minecraft:night_vision","duration":100}]"#,
                r#"[{duration:100,id:"minecraft:night_vision"}]"#,
            ),
            (
                "fireworks flight 1 none",
                component(Component::Fireworks(Fireworks {
                    flight_duration: 1,
                    explosions: Vec::new(),
                })),
                -1231709430,
                r#"{"flight_duration":1}"#,
                "{flight_duration:1b}",
            ),
            (
                "fireworks flight 2 star [red] fade [blue] trail",
                component(Component::Fireworks(Fireworks {
                    flight_duration: 2,
                    explosions: vec![FireworkExplosion {
                        shape: FireworkShape::Star,
                        colours: vec![0x00FF_0000],
                        fade_colours: vec![0x0000_00FF],
                        trail: true,
                        twinkle: false,
                    }],
                })),
                -1683449710,
                r#"{"flight_duration":2,"explosions":[{"shape":"star","colors":[16711680],"fade_colors":[255],"has_trail":true}]}"#,
                r#"{explosions:[{colors:[16711680],fade_colors:[255],has_trail:1b,shape:"star"}],flight_duration:2b}"#,
            ),
            (
                "tooltip_display hide false [enchantments, damage]",
                component(Component::TooltipDisplay(TooltipDisplay {
                    hide_tooltip: false,
                    hidden_components: vec![
                        "minecraft:enchantments".into(),
                        "minecraft:damage".into(),
                    ],
                })),
                805869483,
                r#"{"hidden_components":["minecraft:enchantments","minecraft:damage"]}"#,
                r#"{hidden_components:["minecraft:enchantments","minecraft:damage"]}"#,
            ),
            (
                "tooltip_display hide true []",
                component(Component::TooltipDisplay(TooltipDisplay {
                    hide_tooltip: true,
                    hidden_components: Vec::new(),
                })),
                14016722,
                r#"{"hide_tooltip":true}"#,
                "{hide_tooltip:1b}",
            ),
            (
                "enchantments sharpness 3 unbreaking 1",
                component(Component::Enchantments(vec![
                    ("minecraft:sharpness".into(), 3),
                    ("minecraft:unbreaking".into(), 1),
                ])),
                -973988117,
                r#"{"minecraft:sharpness":3,"minecraft:unbreaking":1}"#,
                r#"{"minecraft:sharpness":3,"minecraft:unbreaking":1}"#,
            ),
            (
                "enchantments empty",
                component(Component::Enchantments(Vec::new())),
                -982207288,
                "{}",
                "{}",
            ),
            (
                "banner_patterns stripe_bottom red, cross white",
                component(Component::BannerPatterns(vec![
                    BannerLayer {
                        pattern: "minecraft:stripe_bottom".into(),
                        colour: "red".into(),
                    },
                    BannerLayer {
                        pattern: "minecraft:cross".into(),
                        colour: "white".into(),
                    },
                ])),
                76609243,
                r#"[{"pattern":"minecraft:stripe_bottom","color":"red"},{"pattern":"minecraft:cross","color":"white"}]"#,
                r#"[{color:"red",pattern:"minecraft:stripe_bottom"},{color:"white",pattern:"minecraft:cross"}]"#,
            ),
            (
                "charged_projectiles empty",
                component(Component::ChargedProjectiles(Vec::new())),
                -1978007022,
                "[]",
                "[]",
            ),
            (
                "charged_projectiles arrow",
                component(Component::ChargedProjectiles(vec![ItemStack::new(
                    item("minecraft:arrow"),
                    1,
                )])),
                1401819792,
                r#"[{"id":"minecraft:arrow"}]"#,
                r#"[{id:"minecraft:arrow"}]"#,
            ),
            (
                "charged_projectiles tipped arrow poison, rocket",
                {
                    let mut poison = Components::NONE;
                    poison.set(Component::PotionContents(PotionContents {
                        potion: Some("minecraft:poison".into()),
                        ..PotionContents::default()
                    }));
                    Value::List(vec![
                        stack_structure(&ItemStack {
                            item: item("minecraft:tipped_arrow"),
                            count: 1,
                            components: poison,
                        }),
                        stack_structure(&ItemStack {
                            item: item("minecraft:firework_rocket"),
                            count: 1,
                            components: Components::NONE,
                        }),
                    ])
                },
                343949924,
                r#"[{"id":"minecraft:tipped_arrow","components":{"minecraft:potion_contents":{"potion":"minecraft:poison"}}},{"id":"minecraft:firework_rocket"}]"#,
                r#"[{components:{"minecraft:potion_contents":{potion:"minecraft:poison"}},id:"minecraft:tipped_arrow"},{id:"minecraft:firework_rocket"}]"#,
            ),
        ]
    }

    #[test]
    fn every_answer_of_the_games_has_its_json_its_nbt_and_its_hash() {
        let answers = answers();
        assert_eq!(answers.len(), 25, "a line of answers.tsv each");
        for (what, value, hash, json, nbt) in answers {
            let wanted: serde_json::Value = serde_json::from_str(json).expect("the game's JSON");
            assert_eq!(as_json(&value), wanted, "{what}: JSON");
            assert_eq!(snbt(&value), nbt, "{what}: NBT");
            assert_eq!(hash_value(&value) as i32, hash, "{what}: hash");
        }
    }

    #[test]
    fn a_components_hash_is_that_of_its_structure() {
        assert_eq!(hash(&Component::Damage(5)), 645064431);
        assert_eq!(hash(&Component::IntangibleProjectile), -982207288);
        assert_eq!(
            hash(&Component::Rarity(Rarity::Epic)),
            hash_value(&structure(&Component::Rarity(Rarity::Epic))) as i32
        );
    }

    #[test]
    fn a_maps_hash_does_not_depend_on_the_order_of_its_entries() {
        let one = Value::Map(vec![
            ("a".into(), Value::Int(1)),
            ("b".into(), Value::Bool(true)),
            ("c".into(), Value::String("x".into())),
        ]);
        let Value::Map(mut entries) = one.clone() else {
            unreachable!()
        };
        entries.reverse();
        assert_eq!(hash_value(&one), hash_value(&Value::Map(entries)));
    }

    #[test]
    fn values_of_different_types_hash_apart() {
        let values = [
            Value::Empty,
            Value::Byte(1),
            Value::Short(1),
            Value::Int(1),
            Value::Long(1),
            Value::Float(1.0),
            Value::Double(1.0),
            Value::Bool(true),
            Value::String("1".into()),
            Value::List(vec![Value::Int(1)]),
            Value::Map(Vec::new()),
            Value::List(Vec::new()),
        ];
        for (at, one) in values.iter().enumerate() {
            for other in &values[at + 1..] {
                assert_ne!(hash_value(one), hash_value(other), "{one:?} and {other:?}");
            }
        }
    }

    #[test]
    fn a_string_is_hashed_by_its_utf16_units() {
        // One unit for an e with an accent and two for an emoji, each two bytes, after
        // the tag and the length in units.
        let mut bytes = vec![TAG_STRING];
        bytes.extend(3i32.to_le_bytes());
        for unit in "\u{e9}\u{1F600}".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        assert_eq!(
            hash_value(&Value::String("\u{e9}\u{1F600}".into())),
            crc32c(&bytes)
        );
    }

    #[test]
    fn crc32c_gives_the_standard_check_value() {
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn every_kind_has_a_hash() {
        // A cheap guard that no kind's structure panics.
        for kind in Kind::ALL {
            let component = match kind {
                Kind::PotionContents => Component::PotionContents(PotionContents::default()),
                Kind::PotionDurationScale => Component::PotionDurationScale(Float(0.125)),
                Kind::ChargedProjectiles => Component::ChargedProjectiles(Vec::new()),
                Kind::Enchantments => Component::Enchantments(Vec::new()),
                Kind::UseCooldown => Component::UseCooldown(UseCooldown {
                    seconds: Float(1.0),
                    group: None,
                }),
                Kind::Fireworks => Component::Fireworks(Fireworks::default()),
                Kind::Damage => Component::Damage(1),
                Kind::IntangibleProjectile => Component::IntangibleProjectile,
                Kind::BannerPatterns => Component::BannerPatterns(Vec::new()),
                Kind::ItemName => Component::ItemName(r#""x""#.into()),
                Kind::Rarity => Component::Rarity(Rarity::Rare),
                Kind::TooltipDisplay => Component::TooltipDisplay(TooltipDisplay::default()),
                Kind::SuspiciousStewEffects => Component::SuspiciousStewEffects(Vec::new()),
                Kind::OminousBottleAmplifier => Component::OminousBottleAmplifier(0),
                Kind::ChickenVariant => Component::ChickenVariant("minecraft:temperate".into()),
            };
            assert_eq!(component.kind(), kind);
            hash(&component);
        }
    }
}
