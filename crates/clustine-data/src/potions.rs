//! The potions: what each gives when drunk, thrown or shot on an arrow.
//!
//! Written by hand from the game's `Potions` class, since the data generator reports a
//! potion's name but not its effects: the static initialiser of
//! `net.minecraft.world.item.alchemy.Potions` (`javap -p -c -constants`, lines 114–905
//! of the dump in `n7k-javap`), where each potion is
//! `register(PotionIds.X, new Potion(name, new MobEffectInstance(effect, duration[,
//! amplifier])...))`. The table is in the order of registration, which is the
//! `minecraft:potion` registry's: a potion's place in [`POTIONS`] is its id.

use crate::EffectInstance;

/// A potion of the game's `Potion`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Potion {
    /// Its name in the registry, such as `minecraft:long_swiftness`.
    pub name: &'static str,
    /// `Potion.name`: what the translation keys of the potion items are made of, shared
    /// by a potion's long and strong forms (`item.minecraft.potion.effect.swiftness`).
    pub translation: &'static str,
    /// What it gives, in the game's order. The constructor that takes no amplifier
    /// gives 0, and none is ambient, all show particles and an icon.
    pub effects: &'static [EffectInstance],
}

impl Potion {
    /// The potion of this name, if there is one.
    pub fn named(name: &str) -> Option<&'static Potion> {
        POTIONS.iter().find(|potion| potion.name == name)
    }

    /// The potion's id in the `minecraft:potion` registry, if there is one of this name.
    pub fn id_of(name: &str) -> Option<usize> {
        POTIONS.iter().position(|potion| potion.name == name)
    }
}

const fn potion(
    name: &'static str,
    translation: &'static str,
    effects: &'static [EffectInstance],
) -> Potion {
    Potion {
        name,
        translation,
        effects,
    }
}

const fn effect(name: &'static str, duration: i32, amplifier: i32) -> EffectInstance {
    EffectInstance::new(name, duration, amplifier)
}

/// Every potion, by its id.
pub static POTIONS: [Potion; 46] = [
    potion("minecraft:water", "water", &[]),
    potion("minecraft:mundane", "mundane", &[]),
    potion("minecraft:thick", "thick", &[]),
    potion("minecraft:awkward", "awkward", &[]),
    potion(
        "minecraft:night_vision",
        "night_vision",
        &[effect("minecraft:night_vision", 3600, 0)],
    ),
    potion(
        "minecraft:long_night_vision",
        "night_vision",
        &[effect("minecraft:night_vision", 9600, 0)],
    ),
    potion(
        "minecraft:invisibility",
        "invisibility",
        &[effect("minecraft:invisibility", 3600, 0)],
    ),
    potion(
        "minecraft:long_invisibility",
        "invisibility",
        &[effect("minecraft:invisibility", 9600, 0)],
    ),
    potion(
        "minecraft:leaping",
        "leaping",
        &[effect("minecraft:jump_boost", 3600, 0)],
    ),
    potion(
        "minecraft:long_leaping",
        "leaping",
        &[effect("minecraft:jump_boost", 9600, 0)],
    ),
    potion(
        "minecraft:strong_leaping",
        "leaping",
        &[effect("minecraft:jump_boost", 1800, 1)],
    ),
    potion(
        "minecraft:fire_resistance",
        "fire_resistance",
        &[effect("minecraft:fire_resistance", 3600, 0)],
    ),
    potion(
        "minecraft:long_fire_resistance",
        "fire_resistance",
        &[effect("minecraft:fire_resistance", 9600, 0)],
    ),
    potion(
        "minecraft:swiftness",
        "swiftness",
        &[effect("minecraft:speed", 3600, 0)],
    ),
    potion(
        "minecraft:long_swiftness",
        "swiftness",
        &[effect("minecraft:speed", 9600, 0)],
    ),
    potion(
        "minecraft:strong_swiftness",
        "swiftness",
        &[effect("minecraft:speed", 1800, 1)],
    ),
    potion(
        "minecraft:slowness",
        "slowness",
        &[effect("minecraft:slowness", 1800, 0)],
    ),
    potion(
        "minecraft:long_slowness",
        "slowness",
        &[effect("minecraft:slowness", 4800, 0)],
    ),
    potion(
        "minecraft:strong_slowness",
        "slowness",
        &[effect("minecraft:slowness", 400, 3)],
    ),
    potion(
        "minecraft:turtle_master",
        "turtle_master",
        &[
            effect("minecraft:slowness", 400, 3),
            effect("minecraft:resistance", 400, 2),
        ],
    ),
    potion(
        "minecraft:long_turtle_master",
        "turtle_master",
        &[
            effect("minecraft:slowness", 800, 3),
            effect("minecraft:resistance", 800, 2),
        ],
    ),
    potion(
        "minecraft:strong_turtle_master",
        "turtle_master",
        &[
            effect("minecraft:slowness", 400, 5),
            effect("minecraft:resistance", 400, 3),
        ],
    ),
    potion(
        "minecraft:water_breathing",
        "water_breathing",
        &[effect("minecraft:water_breathing", 3600, 0)],
    ),
    potion(
        "minecraft:long_water_breathing",
        "water_breathing",
        &[effect("minecraft:water_breathing", 9600, 0)],
    ),
    potion(
        "minecraft:healing",
        "healing",
        &[effect("minecraft:instant_health", 1, 0)],
    ),
    potion(
        "minecraft:strong_healing",
        "healing",
        &[effect("minecraft:instant_health", 1, 1)],
    ),
    potion(
        "minecraft:harming",
        "harming",
        &[effect("minecraft:instant_damage", 1, 0)],
    ),
    potion(
        "minecraft:strong_harming",
        "harming",
        &[effect("minecraft:instant_damage", 1, 1)],
    ),
    potion(
        "minecraft:poison",
        "poison",
        &[effect("minecraft:poison", 900, 0)],
    ),
    potion(
        "minecraft:long_poison",
        "poison",
        &[effect("minecraft:poison", 1800, 0)],
    ),
    potion(
        "minecraft:strong_poison",
        "poison",
        &[effect("minecraft:poison", 432, 1)],
    ),
    potion(
        "minecraft:regeneration",
        "regeneration",
        &[effect("minecraft:regeneration", 900, 0)],
    ),
    potion(
        "minecraft:long_regeneration",
        "regeneration",
        &[effect("minecraft:regeneration", 1800, 0)],
    ),
    potion(
        "minecraft:strong_regeneration",
        "regeneration",
        &[effect("minecraft:regeneration", 450, 1)],
    ),
    potion(
        "minecraft:strength",
        "strength",
        &[effect("minecraft:strength", 3600, 0)],
    ),
    potion(
        "minecraft:long_strength",
        "strength",
        &[effect("minecraft:strength", 9600, 0)],
    ),
    potion(
        "minecraft:strong_strength",
        "strength",
        &[effect("minecraft:strength", 1800, 1)],
    ),
    potion(
        "minecraft:weakness",
        "weakness",
        &[effect("minecraft:weakness", 1800, 0)],
    ),
    potion(
        "minecraft:long_weakness",
        "weakness",
        &[effect("minecraft:weakness", 4800, 0)],
    ),
    potion(
        "minecraft:luck",
        "luck",
        &[effect("minecraft:luck", 6000, 0)],
    ),
    potion(
        "minecraft:slow_falling",
        "slow_falling",
        &[effect("minecraft:slow_falling", 1800, 0)],
    ),
    potion(
        "minecraft:long_slow_falling",
        "slow_falling",
        &[effect("minecraft:slow_falling", 4800, 0)],
    ),
    potion(
        "minecraft:wind_charged",
        "wind_charged",
        &[effect("minecraft:wind_charged", 3600, 0)],
    ),
    potion(
        "minecraft:weaving",
        "weaving",
        &[effect("minecraft:weaving", 3600, 0)],
    ),
    potion(
        "minecraft:oozing",
        "oozing",
        &[effect("minecraft:oozing", 3600, 0)],
    ),
    potion(
        "minecraft:infested",
        "infested",
        &[effect("minecraft:infested", 3600, 0)],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The `minecraft:potion` registry by id, as `registries.json` of 26.3 has it
    /// (copied from `clustine-protocol`'s generated `potion::NAMES`, which this crate
    /// does not depend on).
    const REGISTRY: [&str; 46] = [
        "minecraft:water",
        "minecraft:mundane",
        "minecraft:thick",
        "minecraft:awkward",
        "minecraft:night_vision",
        "minecraft:long_night_vision",
        "minecraft:invisibility",
        "minecraft:long_invisibility",
        "minecraft:leaping",
        "minecraft:long_leaping",
        "minecraft:strong_leaping",
        "minecraft:fire_resistance",
        "minecraft:long_fire_resistance",
        "minecraft:swiftness",
        "minecraft:long_swiftness",
        "minecraft:strong_swiftness",
        "minecraft:slowness",
        "minecraft:long_slowness",
        "minecraft:strong_slowness",
        "minecraft:turtle_master",
        "minecraft:long_turtle_master",
        "minecraft:strong_turtle_master",
        "minecraft:water_breathing",
        "minecraft:long_water_breathing",
        "minecraft:healing",
        "minecraft:strong_healing",
        "minecraft:harming",
        "minecraft:strong_harming",
        "minecraft:poison",
        "minecraft:long_poison",
        "minecraft:strong_poison",
        "minecraft:regeneration",
        "minecraft:long_regeneration",
        "minecraft:strong_regeneration",
        "minecraft:strength",
        "minecraft:long_strength",
        "minecraft:strong_strength",
        "minecraft:weakness",
        "minecraft:long_weakness",
        "minecraft:luck",
        "minecraft:slow_falling",
        "minecraft:long_slow_falling",
        "minecraft:wind_charged",
        "minecraft:weaving",
        "minecraft:oozing",
        "minecraft:infested",
    ];

    fn effects_of(name: &str) -> Vec<(&'static str, i32, i32)> {
        Potion::named(name)
            .unwrap_or_else(|| panic!("no potion {name}"))
            .effects
            .iter()
            .map(|effect| (effect.effect, effect.duration, effect.amplifier))
            .collect()
    }

    #[test]
    fn the_potions_are_in_the_order_of_the_registry() {
        let names: Vec<&str> = POTIONS.iter().map(|potion| potion.name).collect();
        assert_eq!(names, REGISTRY);
        assert_eq!(Potion::id_of("minecraft:swiftness"), Some(13));
        assert_eq!(Potion::id_of("minecraft:infested"), Some(45));
        assert_eq!(Potion::id_of("minecraft:fizzy"), None);
    }

    #[test]
    fn a_potion_s_long_and_strong_forms_change_duration_and_amplifier_but_not_the_translation() {
        assert_eq!(
            effects_of("minecraft:swiftness"),
            [("minecraft:speed", 3600, 0)]
        );
        assert_eq!(
            effects_of("minecraft:long_swiftness"),
            [("minecraft:speed", 9600, 0)]
        );
        assert_eq!(
            effects_of("minecraft:strong_swiftness"),
            [("minecraft:speed", 1800, 1)]
        );
        for name in [
            "minecraft:swiftness",
            "minecraft:long_swiftness",
            "minecraft:strong_swiftness",
        ] {
            assert_eq!(Potion::named(name).unwrap().translation, "swiftness");
        }
    }

    #[test]
    fn healing_and_harming_are_instant_and_poison_lasts() {
        assert_eq!(
            effects_of("minecraft:healing"),
            [("minecraft:instant_health", 1, 0)]
        );
        assert_eq!(
            effects_of("minecraft:strong_harming"),
            [("minecraft:instant_damage", 1, 1)]
        );
        assert_eq!(
            effects_of("minecraft:harming"),
            [("minecraft:instant_damage", 1, 0)]
        );
        assert_eq!(
            effects_of("minecraft:poison"),
            [("minecraft:poison", 900, 0)]
        );
        assert_eq!(
            effects_of("minecraft:strong_poison"),
            [("minecraft:poison", 432, 1)]
        );
    }

    #[test]
    fn the_turtle_master_gives_slowness_then_resistance() {
        assert_eq!(
            effects_of("minecraft:turtle_master"),
            [
                ("minecraft:slowness", 400, 3),
                ("minecraft:resistance", 400, 2)
            ]
        );
        assert_eq!(
            effects_of("minecraft:strong_turtle_master"),
            [
                ("minecraft:slowness", 400, 5),
                ("minecraft:resistance", 400, 3)
            ]
        );
    }

    #[test]
    fn the_four_potions_of_1_21_give_their_own_effects_for_three_minutes() {
        for name in ["oozing", "weaving", "infested", "wind_charged"] {
            let potion = Potion::named(&format!("minecraft:{name}")).unwrap();
            assert_eq!(potion.translation, name);
            assert_eq!(
                potion.effects,
                [EffectInstance {
                    effect: Box::leak(format!("minecraft:{name}").into_boxed_str()),
                    duration: 3600,
                    amplifier: 0,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                }]
            );
        }
    }

    #[test]
    fn the_base_potions_give_nothing() {
        for name in ["water", "mundane", "thick", "awkward"] {
            assert!(effects_of(&format!("minecraft:{name}")).is_empty());
        }
    }
}
