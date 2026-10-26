//! The byte strings here are written by hand from the game's stream codecs (see
//! `values.rs`), not from Clustine's writers.

use proptest::prelude::*;

use super::*;
use crate::nbt::Nbt;
use crate::text::Text;

fn value_bytes(component: &WireComponent) -> Vec<u8> {
    let mut writer = Writer::new();
    writer.put_component(component);
    writer.into_bytes()
}

/// Reads `bytes` as one value of `component`'s kind, wholly, and checks that it is
/// `component` and that it is written back as `bytes`.
fn decodes_and_encodes_back(bytes: &[u8], component: &WireComponent) {
    let mut reader = Reader::new(bytes);
    assert_eq!(reader.component(component.id(), 0).as_ref(), Ok(component));
    assert_eq!(reader.finish(), Ok(()));
    assert_eq!(value_bytes(component), bytes);
}

fn patch(added: Vec<WireComponent>, removed: Vec<i32>) -> WireComponents {
    WireComponents { added, removed }
}

fn effect(effect: i32, amplifier: i32, duration: i32) -> EffectInstance {
    EffectInstance {
        effect,
        details: EffectDetails {
            amplifier,
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
            hidden: None,
        },
    }
}

/// One of every kind of the set, with something in every field.
fn every_component() -> Vec<WireComponent> {
    let mut strong = effect(1, 1, 600);
    strong.details.hidden = Some(Box::new(effect(1, 0, 1200).details));
    vec![
        WireComponent::PotionContents(PotionContents {
            potion: Some(17),
            custom_colour: Some(0x00FF_8800),
            custom_effects: vec![strong, effect(30, 0, -1)],
            custom_name: Some("mine".to_owned()),
        }),
        WireComponent::PotionDurationScale(0.25),
        WireComponent::ChargedProjectiles(vec![
            WireStack {
                item: 900,
                count: 1,
                components: patch(vec![WireComponent::Damage(2)], vec![12]),
            },
            WireStack {
                item: 901,
                count: 3,
                components: WireComponents::default(),
            },
        ]),
        WireComponent::Enchantments(vec![(5, 3), (40, 255)]),
        WireComponent::UseCooldown(UseCooldown {
            seconds: 1.0,
            group: Some("minecraft:ender_pearl".to_owned()),
        }),
        WireComponent::Fireworks(Fireworks {
            flight_duration: 3,
            explosions: vec![FireworkExplosion {
                shape: FireworkShape::Creeper,
                colours: vec![0x11_2233, -1],
                fade_colours: vec![7],
                trail: true,
                twinkle: false,
            }],
        }),
        WireComponent::Damage(300),
        WireComponent::IntangibleProjectile,
        WireComponent::BannerPatterns(vec![
            BannerLayer {
                pattern: BannerPattern::Registered(0),
                colour: 15,
            },
            BannerLayer {
                pattern: BannerPattern::Direct {
                    asset_id: "minecraft:flow".to_owned(),
                    translation_key: "block.minecraft.banner.flow".to_owned(),
                },
                colour: 0,
            },
        ]),
        WireComponent::ItemName(Text::translatable("block.minecraft.ominous_banner")),
        WireComponent::Rarity(Rarity::Uncommon),
        WireComponent::TooltipDisplay(TooltipDisplay {
            hide_tooltip: false,
            hidden_components: vec![data_component_type::BANNER_PATTERNS, 0],
        }),
        WireComponent::SuspiciousStewEffects(vec![(15, 160), (3, 100)]),
        WireComponent::OminousBottleAmplifier(4),
        WireComponent::ChickenVariant(2),
    ]
}

#[test]
fn every_kind_of_the_set_is_one_of_the_values_and_none_twice() {
    let ids: Vec<i32> = every_component().iter().map(WireComponent::id).collect();
    assert_eq!(ids, WireComponent::IDS);
    let names: Vec<&str> = ids
        .iter()
        .map(|id| data_component_type::NAMES[*id as usize])
        .collect();
    assert_eq!(names[0], "minecraft:potion_contents");
    assert_eq!(names[14], "minecraft:chicken/variant");
    assert_eq!(potion::NAMES[0], "minecraft:water");
    assert_eq!(mob_effect::NAMES[0], "minecraft:speed");
}

#[test]
fn every_component_round_trips_as_a_server_and_as_a_client_sends_it() {
    for component in every_component() {
        let components = patch(vec![component], vec![]);

        let mut writer = Writer::new();
        writer.put_components(&components);
        let bytes = writer.into_bytes();
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.components_clientbound().as_ref(), Ok(&components));
        assert_eq!(reader.finish(), Ok(()));

        let mut writer = Writer::new();
        writer.put_components_serverbound(&components);
        let bytes = writer.into_bytes();
        let mut reader = Reader::new(&bytes);
        let read = reader.components_serverbound().unwrap();
        assert_eq!(read.components, components);
        assert!(read.dropped.is_empty());
        assert_eq!(reader.finish(), Ok(()));
    }
}

#[test]
fn a_patch_of_every_component_round_trips_whole() {
    let components = patch(every_component(), vec![1, data_component_type::DAMAGE]);
    let mut writer = Writer::new();
    writer.put_components(&components);
    let bytes = writer.into_bytes();
    assert_eq!(
        Reader::new(&bytes).components_clientbound(),
        Ok(components.clone())
    );
    let mut writer = Writer::new();
    writer.put_components_serverbound(&components);
    let bytes = writer.into_bytes();
    let read = Reader::new(&bytes).components_serverbound().unwrap();
    assert_eq!(read.components, components);
}

#[test]
fn a_potion_by_id_alone_known_answer() {
    // Potion present, id 5; no colour; no effects; no name.
    let component = WireComponent::PotionContents(PotionContents {
        potion: Some(5),
        ..PotionContents::default()
    });
    decodes_and_encodes_back(&[1, 5, 0, 0, 0], &component);
}

#[test]
fn a_potion_with_a_colour_and_two_effects_known_answer() {
    let bytes = [
        0, // no potion
        1, 0x00, 0xFF, 0x00, 0x00, // colour 0xFF0000 as an int
        2,    // two effects
        1, 0, 0xC8, 0x01, 0, 1, 1, 0, // speed: amplifier 0, 200 ticks, shown, no hidden
        9, 2, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 1, 0, 0, 0, // 9: amplifier 2, endless, ambient
        0, // no name
    ];
    let mut endless = effect(9, 2, -1);
    endless.details.ambient = true;
    endless.details.visible = false;
    endless.details.show_icon = false;
    let component = WireComponent::PotionContents(PotionContents {
        potion: None,
        custom_colour: Some(0xFF_0000),
        custom_effects: vec![effect(1, 0, 200), endless],
        custom_name: None,
    });
    decodes_and_encodes_back(&bytes, &component);
}

#[test]
fn a_hidden_effect_follows_its_stronger_one() {
    let bytes = [1, 1, 20, 0, 1, 1, 1, 0, 40, 0, 1, 1, 0];
    let mut strong = effect(1, 1, 20);
    strong.details.hidden = Some(Box::new(effect(1, 0, 40).details));
    assert_eq!(Reader::new(&bytes).effect_instance(), Ok(strong.clone()));
    let mut writer = Writer::new();
    writer.put_effect_instance(&strong);
    assert_eq!(writer.into_bytes(), bytes);
}

#[test]
fn two_enchantments_known_answer() {
    // Two: enchantment 10 at level 3, enchantment 200 at level 1.
    let bytes = [2, 10, 3, 0xC8, 0x01, 1];
    decodes_and_encodes_back(
        &bytes,
        &WireComponent::Enchantments(vec![(10, 3), (200, 1)]),
    );
}

#[test]
fn a_rocket_with_two_explosions_known_answer() {
    let bytes = [
        2, // flight duration
        2, // two explosions
        1, 1, 0x00, 0x11, 0x22, 0x33, 0, 1, 0, // large ball, one colour, no fade, trail
        4, 2, 0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF, 1, 0x00, 0x00, 0x00, 0x2A, 0,
        1, // burst, colours 1 and -1, fade 42, twinkle
    ];
    let component = WireComponent::Fireworks(Fireworks {
        flight_duration: 2,
        explosions: vec![
            FireworkExplosion {
                shape: FireworkShape::LargeBall,
                colours: vec![0x11_2233],
                fade_colours: vec![],
                trail: true,
                twinkle: false,
            },
            FireworkExplosion {
                shape: FireworkShape::Burst,
                colours: vec![1, -1],
                fade_colours: vec![42],
                trail: false,
                twinkle: true,
            },
        ],
    });
    decodes_and_encodes_back(&bytes, &component);
}

#[test]
fn banner_layers_known_answer() {
    let mut bytes = vec![
        2, // two layers
        4, 14, // pattern 3 of the registry (its id plus one), red
        0,  // a pattern given whole
        11,
    ];
    bytes.extend_from_slice(b"minecraft:x");
    bytes.push(3);
    bytes.extend_from_slice(b"a.b");
    bytes.push(15); // black
    let component = WireComponent::BannerPatterns(vec![
        BannerLayer {
            pattern: BannerPattern::Registered(3),
            colour: 14,
        },
        BannerLayer {
            pattern: BannerPattern::Direct {
                asset_id: "minecraft:x".to_owned(),
                translation_key: "a.b".to_owned(),
            },
            colour: 15,
        },
    ]);
    decodes_and_encodes_back(&bytes, &component);
}

#[test]
fn a_tooltip_display_hiding_two_kinds_known_answer() {
    let component = WireComponent::TooltipDisplay(TooltipDisplay {
        hide_tooltip: false,
        hidden_components: vec![data_component_type::ENCHANTMENTS, 53],
    });
    decodes_and_encodes_back(&[0, 2, 13, 53], &component);
}

#[test]
fn the_small_values_known_answers() {
    decodes_and_encodes_back(
        &[0x3E, 0x80, 0, 0],
        &WireComponent::PotionDurationScale(0.25),
    );
    decodes_and_encodes_back(&[0xAC, 0x02], &WireComponent::Damage(300));
    // An empty compound tag, nameless as packets have it.
    decodes_and_encodes_back(&[10, 0], &WireComponent::IntangibleProjectile);
    decodes_and_encodes_back(&[3], &WireComponent::Rarity(Rarity::Epic));
    decodes_and_encodes_back(&[4], &WireComponent::OminousBottleAmplifier(4));
    decodes_and_encodes_back(
        &[1, 15, 0xA0, 0x01],
        &WireComponent::SuspiciousStewEffects(vec![(15, 160)]),
    );
    decodes_and_encodes_back(
        &[0x3F, 0x80, 0, 0, 0],
        &WireComponent::UseCooldown(UseCooldown {
            seconds: 1.0,
            group: None,
        }),
    );
    // The variant's id alone, with no flag before it.
    decodes_and_encodes_back(&[2], &WireComponent::ChickenVariant(2));
    decodes_and_encodes_back(&[0x80, 0x01], &WireComponent::ChickenVariant(128));
    // A string tag, nameless as packets have it.
    let mut name = vec![8, 0, 2];
    name.extend_from_slice(b"Hi");
    decodes_and_encodes_back(&name, &WireComponent::ItemName(Text::literal("Hi")));
}

#[test]
fn a_crossbow_loaded_with_an_arrow_known_answer() {
    // One stack: item 800, count 1, a patch adding damage 1 and removing nothing.
    let bytes = [1, 0xA0, 0x06, 1, 1, 0, 3, 1];
    let component = WireComponent::ChargedProjectiles(vec![WireStack {
        item: 800,
        count: 1,
        components: patch(vec![WireComponent::Damage(1)], vec![]),
    }]);
    decodes_and_encodes_back(&bytes, &component);
}

#[test]
fn ids_out_of_range_of_the_enumerations_read_as_their_first() {
    let mut reader = Reader::new(&[9]);
    assert_eq!(
        reader.component(data_component_type::RARITY, 0),
        Ok(WireComponent::Rarity(Rarity::Common))
    );
    assert_eq!(FireworkShape::of_id(-3), FireworkShape::SmallBall);
    // Pattern 0 of the registry, dye 16: white.
    assert_eq!(
        Reader::new(&[1, 16]).banner_layer(),
        Ok(BannerLayer {
            pattern: BannerPattern::Registered(0),
            colour: 0,
        })
    );
}

#[test]
fn a_patch_of_three_added_and_one_removed_known_answer() {
    let bytes = [
        3, 1, // three added, one removed
        3, 5, // damage 5
        22, 10, 0, // intangible_projectile, an empty compound
        18, 1, 0,  // tooltip_display: hidden, no kinds
        12, // rarity removed
    ];
    let components = patch(
        vec![
            WireComponent::Damage(5),
            WireComponent::IntangibleProjectile,
            WireComponent::TooltipDisplay(TooltipDisplay {
                hide_tooltip: true,
                hidden_components: vec![],
            }),
        ],
        vec![data_component_type::RARITY],
    );
    let mut reader = Reader::new(&bytes);
    assert_eq!(reader.components_clientbound().as_ref(), Ok(&components));
    assert_eq!(reader.finish(), Ok(()));
    let mut writer = Writer::new();
    writer.put_components(&components);
    assert_eq!(writer.into_bytes(), bytes);
}

#[test]
fn an_empty_patch_is_two_noughts() {
    let mut writer = Writer::new();
    writer.put_components(&WireComponents::default());
    assert_eq!(writer.into_bytes(), [0, 0]);
    assert_eq!(
        Reader::new(&[0, 0]).components_clientbound(),
        Ok(WireComponents::default())
    );
}

#[test]
fn a_client_patch_drops_a_component_outside_the_set_by_its_length() {
    let bytes = [
        2, 0, // two added, none removed
        1, 1, 64, // max_stack_size, one byte long: dropped
        3, 1, 7,    // damage, one byte long: 7
        0x2A, // what follows the patch
    ];
    let mut reader = Reader::new(&bytes);
    assert_eq!(
        reader.components_serverbound(),
        Ok(ServerboundComponents {
            components: patch(vec![WireComponent::Damage(7)], vec![]),
            dropped: vec![1],
        })
    );
    assert_eq!(reader.u8(), Ok(0x2A));
}

#[test]
fn a_client_value_is_read_from_its_length_alone() {
    // Damage written in three bytes of which the value takes one: the rest is let be.
    let bytes = [1, 0, 3, 3, 7, 0xEE, 0xEE, 0x2A];
    let mut reader = Reader::new(&bytes);
    let read = reader.components_serverbound().unwrap();
    assert_eq!(read.components.added, [WireComponent::Damage(7)]);
    assert_eq!(reader.u8(), Ok(0x2A));

    // A value longer than its length is cut short.
    let short = [1, 0, 3, 1, 0xAC, 0x02];
    assert!(Reader::new(&short).components_serverbound().is_err());
}

#[test]
fn a_client_crossbow_holding_a_stack_with_a_kind_outside_the_set_is_dropped_whole() {
    // charged_projectiles (51), eight bytes: one stack, item 800 count 1, adding
    // max_stack_size 64.
    let bytes = [1, 0, 51, 8, 1, 0xA0, 0x06, 1, 1, 0, 1, 64];
    let read = Reader::new(&bytes).components_serverbound().unwrap();
    assert!(read.components.is_empty());
    assert_eq!(read.dropped, [data_component_type::CHARGED_PROJECTILES]);
}

#[test]
fn a_server_component_outside_the_set_is_refused_by_its_id() {
    // max_stack_size (1) with its value 64.
    let error = Reader::new(&[1, 0, 1, 64]).components_clientbound();
    assert_eq!(
        error,
        Err(DecodeError::InvalidValue {
            what: OUTSIDE_THE_SET,
            value: 1,
        })
    );
}

#[test]
fn a_kind_id_outside_the_registry_is_refused() {
    let past = data_component_type::NAMES.len() as u8;
    assert!(
        Reader::new(&[1, 0, past, 0])
            .components_clientbound()
            .is_err()
    );
    assert!(Reader::new(&[0, 1, past]).components_clientbound().is_err());
    assert!(
        Reader::new(&[1, 0, past, 0])
            .components_serverbound()
            .is_err()
    );
    assert!(
        Reader::new(&[0, 1, past])
            .component(data_component_type::TOOLTIP_DISPLAY, 0)
            .is_err()
    );
}

#[test]
fn lists_over_the_games_maxima_are_refused() {
    // 257 explosions, after a flight of 1.
    let mut fireworks = vec![1, 0x81, 0x02];
    fireworks.extend(std::iter::repeat_n(0, 257 * 5));
    assert_eq!(
        Reader::new(&fireworks).component(data_component_type::FIREWORKS, 0),
        Err(DecodeError::InvalidValue {
            what: "firework explosions",
            value: 257,
        })
    );
    // 256 are taken.
    let mut most = vec![1, 0x80, 0x02];
    most.extend(std::iter::repeat_n(0, 256 * 5));
    assert!(Reader::new(&most).fireworks().is_ok());

    // 1,025 charged projectiles.
    let mut charged = vec![0x81, 0x08];
    charged.extend(std::iter::repeat_n([1, 1, 0, 0], 1025).flatten());
    assert_eq!(
        Reader::new(&charged).component(data_component_type::CHARGED_PROJECTILES, 0),
        Err(DecodeError::InvalidValue {
            what: "charged projectiles",
            value: 1025,
        })
    );
}

#[test]
fn malformed_values_are_refused() {
    let read = |id, bytes: &[u8]| Reader::new(bytes).component(id, 0);
    // Truncated: a potion said to be there and not.
    assert_eq!(
        read(data_component_type::POTION_CONTENTS, &[1]),
        Err(DecodeError::UnexpectedEnd)
    );
    // An enchantment at level 256.
    assert!(read(data_component_type::ENCHANTMENTS, &[1, 0, 0x80, 0x02]).is_err());
    // A cooldown group that is not an identifier.
    let mut group = vec![0, 0, 0, 0, 1, 2];
    group.extend_from_slice(b"A!");
    assert!(read(data_component_type::USE_COOLDOWN, &group).is_err());
    // A lone end tag for a name.
    assert!(read(data_component_type::ITEM_NAME, &[0]).is_err());
    // A banner pattern of a negative id.
    assert!(
        read(
            data_component_type::BANNER_PATTERNS,
            &[1, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0]
        )
        .is_err()
    );
    // A stack of nothing, or of air, in a crossbow.
    assert!(read(data_component_type::CHARGED_PROJECTILES, &[1, 5, 0, 0, 0]).is_err());
    assert!(read(data_component_type::CHARGED_PROJECTILES, &[1, 0, 1, 0, 0]).is_err());
    // A patch's negative count.
    assert!(
        Reader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0])
            .components_clientbound()
            .is_err()
    );
}

#[test]
fn stacks_nested_past_the_bound_are_refused() {
    // Each level: one charged projectile of item 1, count 1, a patch adding one
    // charged_projectiles holding the next level.
    fn nested(levels: usize) -> Vec<u8> {
        let mut bytes = vec![1, 0, 51];
        for _ in 0..levels {
            bytes.extend_from_slice(&[1, 1, 1, 1, 0, 51]);
        }
        bytes.push(0); // the innermost holds no stack
        bytes
    }
    assert!(
        Reader::new(&nested(MAX_NESTING - 1))
            .components_clientbound()
            .is_ok()
    );
    assert!(
        Reader::new(&nested(MAX_NESTING + 1))
            .components_clientbound()
            .is_err()
    );
}

#[test]
fn an_intangible_marker_is_any_compound_and_nothing_else() {
    let read = |bytes: &[u8]| {
        let mut reader = Reader::new(bytes);
        let value = reader.component(data_component_type::INTANGIBLE_PROJECTILE, 0);
        (value, reader.remaining())
    };
    // A compound holding a byte named "a": `Unit.CODEC` ignores what is there.
    let full = [10, 1, 0, 1, b'a', 7, 0, 0x2A];
    assert_eq!(read(&full), (Ok(WireComponent::IntangibleProjectile), 1));
    // A byte tag is not a map, and a lone end tag is refused by the tag codec.
    assert!(read(&[1, 1]).0.is_err());
    assert!(read(&[0]).0.is_err());
    // Nothing at all is cut short: the marker has bytes on the wire.
    assert_eq!(read(&[]).0, Err(DecodeError::UnexpectedEnd));
}

#[test]
fn a_stack_of_a_negative_count_in_a_crossbow_passes_as_the_games_template_lets_it() {
    // One stack: item 800, count -1, an empty patch.
    let bytes = [1, 0xA0, 0x06, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0, 0];
    let component = WireComponent::ChargedProjectiles(vec![WireStack {
        item: 800,
        count: -1,
        components: WireComponents::default(),
    }]);
    decodes_and_encodes_back(&bytes, &component);
}

#[test]
fn an_enchantment_given_twice_has_the_level_given_last() {
    let read = |bytes: &[u8]| Reader::new(bytes).component(data_component_type::ENCHANTMENTS, 0);
    // Enchantment 10 at 3, 7 at 1, 10 again at 5: 10 keeps its place and takes 5.
    assert_eq!(
        read(&[3, 10, 3, 7, 1, 10, 5]),
        Ok(WireComponent::Enchantments(vec![(10, 5), (7, 1)]))
    );
    // The range is checked on the levels the map keeps: 256 replaced by 1 passes,
    // and 1 replaced by 256 does not.
    assert_eq!(
        read(&[2, 10, 0x80, 0x02, 10, 1]),
        Ok(WireComponent::Enchantments(vec![(10, 1)]))
    );
    assert_eq!(
        read(&[2, 10, 1, 10, 0x80, 0x02]),
        Err(DecodeError::InvalidValue {
            what: "enchantment level",
            value: 256,
        })
    );
    // A negative level is refused too, and nought is taken.
    assert!(read(&[1, 10, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]).is_err());
    assert_eq!(
        read(&[1, 10, 0]),
        Ok(WireComponent::Enchantments(vec![(10, 0)]))
    );
}

#[test]
fn a_kind_hidden_twice_from_the_tooltip_is_kept_once_in_its_first_place() {
    assert_eq!(
        Reader::new(&[0, 3, 13, 53, 13]).component(data_component_type::TOOLTIP_DISPLAY, 0),
        Ok(WireComponent::TooltipDisplay(TooltipDisplay {
            hide_tooltip: false,
            hidden_components: vec![13, 53],
        }))
    );
}

#[test]
fn identifiers_are_parsed_as_the_games_identifier_parses_them() {
    let cooldown = |group: &str| {
        let mut bytes = vec![0x3F, 0x80, 0, 0, 1, group.len() as u8];
        bytes.extend_from_slice(group.as_bytes());
        Reader::new(&bytes).component(data_component_type::USE_COOLDOWN, 0)
    };
    let group = |group: &str| {
        Ok(WireComponent::UseCooldown(UseCooldown {
            seconds: 1.0,
            group: Some(group.to_owned()),
        }))
    };
    // No namespace, an empty one, and the path's own characters are taken as written.
    assert_eq!(cooldown("ender_pearl"), group("ender_pearl"));
    assert_eq!(cooldown(":ender_pearl"), group(":ender_pearl"));
    assert_eq!(cooldown("a-b.c:d/e_f.9"), group("a-b.c:d/e_f.9"));
    // A namespace of `..`, a slash in the namespace, a second colon, an upper-case
    // letter or a space is refused.
    for refused in ["..:x", "a/b:c", "a:b:c", "Minecraft:x", "a: b"] {
        assert!(cooldown(refused).is_err(), "{refused}");
    }
    // `..` alone is a path in the default namespace, not a namespace.
    assert_eq!(cooldown(".."), group(".."));
}

#[test]
fn a_name_that_says_more_is_kept_as_it_came() {
    let value = Nbt::Compound(vec![
        ("text".to_owned(), Nbt::String("Banner".to_owned())),
        ("color".to_owned(), Nbt::String("gold".to_owned())),
    ]);
    let component = WireComponent::ItemName(Text::from_nbt(value.clone()));
    let bytes = value_bytes(&component);
    assert_eq!(
        Reader::new(&bytes).component(data_component_type::ITEM_NAME, 0),
        Ok(component)
    );
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
        let _ = Reader::new(&bytes).components_clientbound();
        let _ = Reader::new(&bytes).components_serverbound();
        for id in WireComponent::IDS {
            let _ = Reader::new(&bytes).component(id, 0);
        }
    }

    #[test]
    // Each enchantment once: the game keeps one level for each (see
    // `an_enchantment_given_twice_has_the_level_given_last`).
    fn damage_and_enchantments_round_trip(damage: i32, levels in prop::collection::btree_map(0..2000i32, 0..=255i32, 0..8)) {
        let components = patch(
            vec![WireComponent::Enchantments(levels.into_iter().collect()), WireComponent::Damage(damage)],
            vec![],
        );
        let mut writer = Writer::new();
        writer.put_components(&components);
        let bytes = writer.into_bytes();
        prop_assert_eq!(Reader::new(&bytes).components_clientbound(), Ok(components));
    }
}
