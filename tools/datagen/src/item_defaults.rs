//! The default components of an item that N9 reads (`docs/groundwork/n9-plan.md`,
//! sections 3.1 and 5.5): a potion's, a projectile's, a cooldown, and how a food or a
//! drink is eaten and what it does. Read from the data generator's report of an item's
//! components, each by its codec as the dumps in `n9-javap` have it, and rendered as the
//! values of `clustine_data::ItemComponents`.
//!
//! A shape the codecs allow but no item's default has (an inline sound event, a
//! hidden effect, a default firework with explosions, a remainder with components) is
//! refused rather than guessed at, so that a game update that brings one stops the
//! generator.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value};

/// An effect as `MobEffectInstance.CODEC` writes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectReport {
    pub effect: String,
    pub duration: i32,
    pub amplifier: i32,
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
}

/// A default `minecraft:potion_contents` that holds something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PotionReport {
    pub potion: Option<String>,
    pub colour: Option<i32>,
    pub effects: Vec<EffectReport>,
    pub name: Option<String>,
}

/// One of a consumable's `on_consume_effects`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsumeEffectReport {
    /// The effects and the probability's float bits.
    Apply(Vec<EffectReport>, u32),
    /// The effects by name, or a tag's name without its `#`.
    Remove(Result<Vec<String>, String>),
    ClearAll,
    /// The diameter's float bits and whether the particles are directional.
    Teleport(u32, bool),
    PlaySound(String),
}

/// A `minecraft:consumable`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumableReport {
    /// The float bits of `consume_seconds`.
    pub seconds: u32,
    /// The variant of `clustine_data::UseAnimation`.
    pub animation: &'static str,
    pub sound: String,
    pub particles: bool,
    pub effects: Vec<ConsumeEffectReport>,
}

/// What of an item's defaults N9 reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    pub potion_contents: Option<PotionReport>,
    /// Float bits.
    pub potion_duration_scale: Option<u32>,
    pub damage: Option<i32>,
    /// The seconds' float bits and the group.
    pub use_cooldown: Option<(u32, Option<String>)>,
    pub fireworks_flight_duration: Option<i32>,
    pub charged_projectiles: bool,
    pub chicken_variant: Option<String>,
    pub consumable: Option<ConsumableReport>,
    /// Nutrition, saturation's float bits, and whether it can always be eaten.
    pub food: Option<(i32, u32, bool)>,
    /// The item by name, and the count.
    pub use_remainder: Option<(String, i32)>,
    pub ominous_bottle_amplifier: Option<i32>,
}

/// `ItemUseAnimation`'s serialised names, each with the variant of
/// `clustine_data::UseAnimation` it becomes: the constants the dumps name, by the
/// lower-case form the game's `StringRepresentable` enums are written in
/// (**Inferred**: the enum is not dumped; "drink" is in the reports).
const USE_ANIMATIONS: [(&str, &str); 9] = [
    ("none", "None"),
    ("eat", "Eat"),
    ("drink", "Drink"),
    ("block", "Block"),
    ("bow", "Bow"),
    ("trident", "Trident"),
    ("spear", "Spear"),
    ("crossbow", "Crossbow"),
    ("toot_horn", "TootHorn"),
];

/// `Consumable.CODEC`'s defaults (n9 Consumable 350–380): 1.6 seconds, eating, the
/// generic eating sound and crumbs.
const DEFAULT_CONSUME_SECONDS: f32 = 1.6;
const DEFAULT_CONSUME_SOUND: &str = "minecraft:entity.generic.eat";

/// `TeleportRandomlyConsumeEffect.DEFAULT_DIAMETER` (n9 TeleportRandomlyConsumeEffect 7).
const DEFAULT_DIAMETER: f32 = 16.0;

/// Reads the defaults of one item from its report's `components`.
pub fn read(components: &Map<String, Value>) -> Result<Defaults> {
    let get = |kind: &str| components.get(&format!("minecraft:{kind}"));
    let mut defaults = Defaults::default();
    if let Some(value) = get("potion_contents") {
        defaults.potion_contents = potion_contents(value).context("the potion contents")?;
    }
    if let Some(value) = get("potion_duration_scale") {
        defaults.potion_duration_scale = Some(float(value, "a potion's duration scale")?);
    }
    if let Some(value) = get("damage") {
        defaults.damage = Some(int(value, "the damage")?);
    }
    if let Some(value) = get("use_cooldown") {
        let object = fields(value, &["seconds", "cooldown_group"], "a use cooldown")?;
        let seconds = float(
            object
                .get("seconds")
                .context("a cooldown without seconds")?,
            "a cooldown's seconds",
        )?;
        let group = object
            .get("cooldown_group")
            .map(|group| name(group, "a cooldown group"))
            .transpose()?;
        defaults.use_cooldown = Some((seconds, group));
    }
    if let Some(value) = get("fireworks") {
        let object = fields(value, &["flight_duration", "explosions"], "fireworks")?;
        ensure!(
            object
                .get("explosions")
                .is_none_or(|explosions| explosions.as_array().is_some_and(Vec::is_empty)),
            "default fireworks with explosions are not understood"
        );
        // `Fireworks.CODEC` gives a flight of 0 where none is written (n9 Fireworks 190).
        defaults.fireworks_flight_duration = Some(
            object
                .get("flight_duration")
                .map_or(Ok(0), |value| int(value, "a flight duration"))?,
        );
    }
    if let Some(value) = get("charged_projectiles") {
        ensure!(
            value.as_array().is_some_and(Vec::is_empty),
            "a default charge that is not empty is not understood"
        );
        defaults.charged_projectiles = true;
    }
    if let Some(value) = get("chicken/variant") {
        defaults.chicken_variant = Some(name(value, "a chicken variant")?);
    }
    if let Some(value) = get("consumable") {
        defaults.consumable = Some(consumable(value).context("the consumable")?);
    }
    if let Some(value) = get("food") {
        let object = fields(
            value,
            &["nutrition", "saturation", "can_always_eat"],
            "a food",
        )?;
        let nutrition = int(
            object
                .get("nutrition")
                .context("a food without nutrition")?,
            "nutrition",
        )?;
        let saturation = float(
            object
                .get("saturation")
                .context("a food without saturation")?,
            "saturation",
        )?;
        let always = object
            .get("can_always_eat")
            .map_or(Ok(false), |value| boolean(value, "can_always_eat"))?;
        defaults.food = Some((nutrition, saturation, always));
    }
    if let Some(value) = get("use_remainder") {
        let object = fields(value, &["id", "count"], "a use remainder")?;
        let item = name(
            object.get("id").context("a remainder without an item")?,
            "a remainder's item",
        )?;
        // `ItemStackTemplate.CODEC` counts 1 where none is written (n9 ItemStackTemplate
        // 281).
        let count = object
            .get("count")
            .map_or(Ok(1), |value| int(value, "a remainder's count"))?;
        defaults.use_remainder = Some((item, count));
    }
    if let Some(value) = get("ominous_bottle_amplifier") {
        defaults.ominous_bottle_amplifier = Some(int(value, "an ominous bottle's amplifier")?);
    }
    if let Some(value) = get("suspicious_stew_effects") {
        // A stew's effects come only from its patch; a default would need a field.
        ensure!(
            value.as_array().is_some_and(Vec::is_empty),
            "default suspicious stew effects are not understood"
        );
    }
    Ok(defaults)
}

/// `PotionContents.CODEC`: a potion's name alone, or an object. An empty object is no
/// default at all.
fn potion_contents(value: &Value) -> Result<Option<PotionReport>> {
    if let Some(potion) = value.as_str() {
        return Ok(Some(PotionReport {
            potion: Some(potion.to_owned()),
            colour: None,
            effects: Vec::new(),
            name: None,
        }));
    }
    let object = fields(
        value,
        &["potion", "custom_color", "custom_effects", "custom_name"],
        "potion contents",
    )?;
    if object.is_empty() {
        return Ok(None);
    }
    Ok(Some(PotionReport {
        potion: object
            .get("potion")
            .map(|value| name(value, "a potion"))
            .transpose()?,
        colour: object
            .get("custom_color")
            .map(|value| int(value, "a potion's colour"))
            .transpose()?,
        effects: match object.get("custom_effects") {
            None => Vec::new(),
            Some(list) => list
                .as_array()
                .context("custom effects that are not a list")?
                .iter()
                .map(effect)
                .collect::<Result<_>>()?,
        },
        name: object
            .get("custom_name")
            .map(|value| name(value, "a potion's own name"))
            .transpose()?,
    }))
}

/// `MobEffectInstance.CODEC`: the effect and its details, whose codec gives amplifier
/// and duration 0, no ambience, particles, and an icon as the particles where they are
/// not written (**Inferred** from the game's `MobEffectInstance$Details`, which is not
/// dumped; every report writes `show_icon`).
fn effect(value: &Value) -> Result<EffectReport> {
    let object = fields(
        value,
        &[
            "id",
            "amplifier",
            "duration",
            "ambient",
            "show_particles",
            "show_icon",
        ],
        "an effect",
    )?;
    let flag = |key: &str, default: bool| {
        object
            .get(key)
            .map_or(Ok(default), |value| boolean(value, key))
    };
    let visible = flag("show_particles", true)?;
    Ok(EffectReport {
        effect: name(
            object.get("id").context("an effect without its id")?,
            "an effect",
        )?,
        duration: object
            .get("duration")
            .map_or(Ok(0), |value| int(value, "an effect's duration"))?,
        amplifier: object
            .get("amplifier")
            .map_or(Ok(0), |value| int(value, "an effect's amplifier"))?,
        ambient: flag("ambient", false)?,
        visible,
        show_icon: flag("show_icon", visible)?,
    })
}

/// `Consumable.CODEC` (n9 Consumable 350–380).
fn consumable(value: &Value) -> Result<ConsumableReport> {
    let object = fields(
        value,
        &[
            "consume_seconds",
            "animation",
            "sound",
            "has_consume_particles",
            "on_consume_effects",
        ],
        "a consumable",
    )?;
    let animation = match object.get("animation") {
        None => "Eat",
        Some(value) => {
            let animation = value.as_str().context("an animation that is not a name")?;
            USE_ANIMATIONS
                .iter()
                .find(|(name, _)| *name == animation)
                .with_context(|| format!("a use animation {animation}"))?
                .1
        }
    };
    let mut effects = Vec::new();
    if let Some(list) = object.get("on_consume_effects") {
        for entry in list
            .as_array()
            .context("consume effects that are not a list")?
        {
            effects.push(consume_effect(entry)?);
        }
    }
    Ok(ConsumableReport {
        seconds: object
            .get("consume_seconds")
            .map_or(Ok(DEFAULT_CONSUME_SECONDS.to_bits()), |value| {
                float(value, "consume_seconds")
            })?,
        animation,
        sound: object
            .get("sound")
            .map_or(Ok(DEFAULT_CONSUME_SOUND.to_owned()), sound)?,
        particles: object
            .get("has_consume_particles")
            .map_or(Ok(true), |value| boolean(value, "has_consume_particles"))?,
        effects,
    })
}

/// One `ConsumeEffect`, by its `type` (n9 ConsumeEffect$Type and the five kinds).
fn consume_effect(value: &Value) -> Result<ConsumeEffectReport> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .context("a consume effect without its type")?;
    Ok(match kind {
        "minecraft:apply_effects" => {
            let object = fields(value, &["type", "effects", "probability"], kind)?;
            let effects = object
                .get("effects")
                .and_then(Value::as_array)
                .context("applied effects that are not a list")?
                .iter()
                .map(effect)
                .collect::<Result<_>>()?;
            // `probability` is a float of 0 to 1, 1 where not written (n9
            // ApplyStatusEffectsConsumeEffect 24–35).
            let probability = object
                .get("probability")
                .map_or(Ok(1.0_f32.to_bits()), |value| float(value, "a probability"))?;
            ensure!(
                (0.0..=1.0).contains(&f32::from_bits(probability)),
                "a probability outside 0 to 1"
            );
            ConsumeEffectReport::Apply(effects, probability)
        }
        "minecraft:remove_effects" => {
            let object = fields(value, &["type", "effects"], kind)?;
            let effects = object.get("effects").context("no effects to remove")?;
            // A `HolderSet`: a tag, one name, or a list of names.
            ConsumeEffectReport::Remove(match effects {
                Value::String(text) => match text.strip_prefix('#') {
                    Some(tag) => Err(tag.to_owned()),
                    None => Ok(vec![text.clone()]),
                },
                Value::Array(list) => Ok(list
                    .iter()
                    .map(|entry| name(entry, "an effect to remove"))
                    .collect::<Result<_>>()?),
                _ => bail!("effects to remove that are neither a name nor a list"),
            })
        }
        "minecraft:clear_all_effects" => {
            fields(value, &["type"], kind)?;
            ConsumeEffectReport::ClearAll
        }
        "minecraft:teleport_randomly" => {
            let object = fields(value, &["type", "diameter", "directional_particles"], kind)?;
            ConsumeEffectReport::Teleport(
                object
                    .get("diameter")
                    .map_or(Ok(DEFAULT_DIAMETER.to_bits()), |value| {
                        float(value, "a diameter")
                    })?,
                object
                    .get("directional_particles")
                    .map_or(Ok(true), |value| boolean(value, "directional_particles"))?,
            )
        }
        "minecraft:play_sound" => {
            let object = fields(value, &["type", "sound"], kind)?;
            ConsumeEffectReport::PlaySound(sound(
                object
                    .get("sound")
                    .context("a sound effect without its sound")?,
            )?)
        }
        _ => bail!("a consume effect {kind}"),
    })
}

/// The object `value` is, refusing a key not in `known`: a field this reader does not
/// know would be dropped without a word.
fn fields<'a>(value: &'a Value, known: &[&str], what: &str) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .with_context(|| format!("{what} that is not an object"))?;
    if let Some(key) = object.keys().find(|key| !known.contains(&key.as_str())) {
        bail!("{what} with a field {key} that is not understood");
    }
    Ok(object)
}

fn name(value: &Value, what: &str) -> Result<String> {
    Ok(value
        .as_str()
        .with_context(|| format!("{what} that is not a name"))?
        .to_owned())
}

/// A sound event by its name; `SoundEvent.CODEC` also takes an inline event, which no
/// default has.
fn sound(value: &Value) -> Result<String> {
    name(value, "a sound other than a sound event's name")
}

fn int(value: &Value, what: &str) -> Result<i32> {
    value
        .as_i64()
        .and_then(|number| i32::try_from(number).ok())
        .with_context(|| format!("{what} that is not an int"))
}

fn boolean(value: &Value, what: &str) -> Result<bool> {
    value
        .as_bool()
        .with_context(|| format!("{what} that is not a boolean"))
}

/// A float the report writes as a decimal, which the game's codec read into a float.
fn float(value: &Value, what: &str) -> Result<u32> {
    Ok((value
        .as_f64()
        .with_context(|| format!("{what} that is not a number"))? as f32)
        .to_bits())
}

/// The registries the rendering checks names against.
pub struct Registries<'a> {
    pub items: &'a [String],
    pub mob_effects: &'a [String],
    pub sound_events: &'a [String],
}

impl Registries<'_> {
    fn effect(&self, name: &str) -> Result<String> {
        ensure!(
            self.mob_effects.iter().any(|known| known == name),
            "{name} is no mob effect"
        );
        Ok(format!("{name:?}"))
    }

    fn sound(&self, name: &str) -> Result<String> {
        ensure!(
            self.sound_events.iter().any(|known| known == name),
            "{name} is no sound event"
        );
        Ok(format!("{name:?}"))
    }
}

/// The fields of `clustine_data::ItemComponents` these defaults give, each with the Rust
/// expression of its value, in the struct's order.
pub fn render(defaults: &Defaults, registries: &Registries) -> Result<Vec<(&'static str, String)>> {
    let some =
        |value: Option<String>| value.map_or("None".to_owned(), |value| format!("Some({value})"));
    let potion_contents = match &defaults.potion_contents {
        None => None,
        Some(potion) => Some(format!(
            "PotionContents {{ potion: {}, custom_colour: {}, custom_effects: &[{}], custom_name: {} }}",
            some(potion.potion.as_ref().map(|name| format!("{name:?}"))),
            some(potion.colour.map(|colour| colour.to_string())),
            effects(&potion.effects, registries)?,
            some(potion.name.as_ref().map(|name| format!("{name:?}"))),
        )),
    };
    let use_cooldown = defaults.use_cooldown.as_ref().map(|(seconds, group)| {
        format!(
            "UseCooldown {{ seconds: {}, group: {} }}",
            float_literal(*seconds),
            some(group.as_ref().map(|group| format!("{group:?}")))
        )
    });
    let consumable = match &defaults.consumable {
        None => None,
        Some(consumable) => {
            let entries = consumable
                .effects
                .iter()
                .map(|entry| consume_effect_literal(entry, registries))
                .collect::<Result<Vec<_>>>()?;
            Some(format!(
                "Consumable {{ consume_seconds: {}, animation: UseAnimation::{}, sound: {}, has_consume_particles: {}, on_consume_effects: &[{}] }}",
                float_literal(consumable.seconds),
                consumable.animation,
                registries.sound(&consumable.sound)?,
                consumable.particles,
                entries.join(", ")
            ))
        }
    };
    let food = defaults.food.map(|(nutrition, saturation, always)| {
        format!(
            "Food {{ nutrition: {nutrition}, saturation: {}, can_always_eat: {always} }}",
            float_literal(saturation)
        )
    });
    let use_remainder = match &defaults.use_remainder {
        None => None,
        Some((item, count)) => {
            let id = registries
                .items
                .iter()
                .position(|known| known == item)
                .with_context(|| format!("the remainder {item} is no item"))?;
            Some(format!("UseRemainder {{ item: {id}, count: {count} }}"))
        }
    };
    Ok(vec![
        ("potion_contents", some(potion_contents)),
        (
            "potion_duration_scale",
            some(defaults.potion_duration_scale.map(float_literal)),
        ),
        (
            "damage",
            some(defaults.damage.map(|damage| damage.to_string())),
        ),
        ("use_cooldown", some(use_cooldown)),
        (
            "fireworks_flight_duration",
            some(
                defaults
                    .fireworks_flight_duration
                    .map(|flight| flight.to_string()),
            ),
        ),
        (
            "charged_projectiles",
            defaults.charged_projectiles.to_string(),
        ),
        (
            "chicken_variant",
            some(
                defaults
                    .chicken_variant
                    .as_ref()
                    .map(|name| format!("{name:?}")),
            ),
        ),
        ("consumable", some(consumable)),
        ("food", some(food)),
        ("use_remainder", some(use_remainder)),
        (
            "ominous_bottle_amplifier",
            some(
                defaults
                    .ominous_bottle_amplifier
                    .map(|amplifier| amplifier.to_string()),
            ),
        ),
    ])
}

/// The types of `clustine_data` a rendering may name, which the generated file imports
/// where it does.
pub const TYPES: [&str; 9] = [
    "ConsumeEffect",
    "Consumable",
    "EffectInstance",
    "EffectSet",
    "Food",
    "PotionContents",
    "UseAnimation",
    "UseCooldown",
    "UseRemainder",
];

fn consume_effect_literal(entry: &ConsumeEffectReport, registries: &Registries) -> Result<String> {
    Ok(match entry {
        ConsumeEffectReport::Apply(list, probability) => format!(
            "ConsumeEffect::ApplyEffects {{ effects: &[{}], probability: {} }}",
            effects(list, registries)?,
            float_literal(*probability)
        ),
        ConsumeEffectReport::Remove(Ok(names)) => {
            let names = names
                .iter()
                .map(|name| registries.effect(name))
                .collect::<Result<Vec<_>>>()?;
            format!(
                "ConsumeEffect::RemoveEffects(EffectSet::Listed(&[{}]))",
                names.join(", ")
            )
        }
        ConsumeEffectReport::Remove(Err(tag)) => {
            format!("ConsumeEffect::RemoveEffects(EffectSet::Tag({tag:?}))")
        }
        ConsumeEffectReport::ClearAll => "ConsumeEffect::ClearAllEffects".to_owned(),
        ConsumeEffectReport::Teleport(diameter, directional) => format!(
            "ConsumeEffect::TeleportRandomly {{ diameter: {}, directional_particles: {directional} }}",
            float_literal(*diameter)
        ),
        ConsumeEffectReport::PlaySound(sound) => {
            format!("ConsumeEffect::PlaySound({})", registries.sound(sound)?)
        }
    })
}

/// The effects as a list of `EffectInstance`s: by its constructor where it is as
/// `new MobEffectInstance(effect, duration, amplifier)` makes one, which every default
/// is.
fn effects(list: &[EffectReport], registries: &Registries) -> Result<String> {
    let rendered = list
        .iter()
        .map(|effect| {
            let name = registries.effect(&effect.effect)?;
            Ok(if !effect.ambient && effect.visible && effect.show_icon {
                format!("EffectInstance::new({name}, {}, {})", effect.duration, effect.amplifier)
            } else {
                format!(
                    "EffectInstance {{ effect: {name}, duration: {}, amplifier: {}, ambient: {}, visible: {}, show_icon: {} }}",
                    effect.duration, effect.amplifier, effect.ambient, effect.visible, effect.show_icon
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join(", "))
}

/// A float literal that reads back as these bits: Rust's shortest decimal of the float,
/// which round-trips, with its point kept so that it is a float.
fn float_literal(bits: u32) -> String {
    let value = f32::from_bits(bits);
    assert!(value.is_finite(), "a float of the data is finite");
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn read_json(text: &str) -> Result<Defaults> {
        let value: Value = serde_json::from_str(text).unwrap();
        read(value.as_object().unwrap())
    }

    /// A fixture of every shape the reports hold, with the codecs' defaults left out
    /// where the reports leave them out.
    const FIXTURE: &str = r##"{
        "minecraft:potion_contents": {},
        "minecraft:potion_duration_scale": 0.25,
        "minecraft:damage": 0,
        "minecraft:use_cooldown": {"seconds": 1.0},
        "minecraft:fireworks": {"flight_duration": 1},
        "minecraft:charged_projectiles": [],
        "minecraft:chicken/variant": "minecraft:cold",
        "minecraft:consumable": {
            "animation": "drink",
            "has_consume_particles": false,
            "sound": "minecraft:item.honey_bottle.drink",
            "on_consume_effects": [
                {"type": "minecraft:apply_effects", "probability": 0.3,
                 "effects": [{"id": "minecraft:hunger", "duration": 600, "show_icon": true},
                             {"id": "minecraft:poison", "duration": 100, "amplifier": 1,
                              "show_particles": false}]},
                {"type": "minecraft:remove_effects", "effects": "minecraft:poison"},
                {"type": "minecraft:remove_effects", "effects": "#minecraft:bad"},
                {"type": "minecraft:clear_all_effects"},
                {"type": "minecraft:teleport_randomly"},
                {"type": "minecraft:play_sound", "sound": "minecraft:item.ominous_bottle.dispose"}
            ]
        },
        "minecraft:food": {"nutrition": 3, "saturation": 1.8000001, "can_always_eat": true},
        "minecraft:use_remainder": {"id": "minecraft:glass_bottle"},
        "minecraft:ominous_bottle_amplifier": 0,
        "minecraft:suspicious_stew_effects": []
    }"##;

    #[test]
    fn every_shape_of_the_reports_is_read_with_the_codecs_defaults() {
        let defaults = read_json(FIXTURE).unwrap();
        assert_eq!(defaults.potion_contents, None);
        assert_eq!(defaults.potion_duration_scale, Some(0.25_f32.to_bits()));
        assert_eq!(defaults.damage, Some(0));
        assert_eq!(defaults.use_cooldown, Some((1.0_f32.to_bits(), None)));
        assert_eq!(defaults.fireworks_flight_duration, Some(1));
        assert!(defaults.charged_projectiles);
        assert_eq!(defaults.chicken_variant.as_deref(), Some("minecraft:cold"));
        assert_eq!(defaults.food, Some((3, 1.8000001_f32.to_bits(), true)));
        assert_eq!(
            defaults.use_remainder,
            Some(("minecraft:glass_bottle".to_owned(), 1))
        );
        assert_eq!(defaults.ominous_bottle_amplifier, Some(0));
        let consumable = defaults.consumable.unwrap();
        assert_eq!(consumable.seconds, 1.6_f32.to_bits());
        assert_eq!(consumable.animation, "Drink");
        assert!(!consumable.particles);
        assert_eq!(
            consumable.effects,
            [
                ConsumeEffectReport::Apply(
                    vec![
                        EffectReport {
                            effect: "minecraft:hunger".to_owned(),
                            duration: 600,
                            amplifier: 0,
                            ambient: false,
                            visible: true,
                            show_icon: true,
                        },
                        EffectReport {
                            effect: "minecraft:poison".to_owned(),
                            duration: 100,
                            amplifier: 1,
                            ambient: false,
                            visible: false,
                            show_icon: false,
                        },
                    ],
                    0.3_f32.to_bits()
                ),
                ConsumeEffectReport::Remove(Ok(vec!["minecraft:poison".to_owned()])),
                ConsumeEffectReport::Remove(Err("minecraft:bad".to_owned())),
                ConsumeEffectReport::ClearAll,
                ConsumeEffectReport::Teleport(16.0_f32.to_bits(), true),
                ConsumeEffectReport::PlaySound("minecraft:item.ominous_bottle.dispose".to_owned()),
            ]
        );
    }

    #[test]
    fn an_item_without_the_components_has_none_of_the_defaults() {
        assert_eq!(read_json("{}").unwrap(), Defaults::default());
        let consumable = read_json(r#"{"minecraft:consumable": {}}"#)
            .unwrap()
            .consumable
            .unwrap();
        assert_eq!(
            (
                consumable.seconds,
                consumable.animation,
                consumable.sound.as_str(),
                consumable.particles
            ),
            (
                1.6_f32.to_bits(),
                "Eat",
                "minecraft:entity.generic.eat",
                true
            )
        );
        assert!(consumable.effects.is_empty());
    }

    #[test]
    fn what_no_default_has_is_refused_rather_than_dropped() {
        for text in [
            r#"{"minecraft:fireworks": {"explosions": [{"shape": "star"}]}}"#,
            r#"{"minecraft:charged_projectiles": [{"id": "minecraft:arrow"}]}"#,
            r#"{"minecraft:use_remainder": {"id": "minecraft:bowl", "components": {}}}"#,
            r#"{"minecraft:consumable": {"sound": {"sound_id": "a"}}}"#,
            r#"{"minecraft:consumable": {"animation": "juggle"}}"#,
            r#"{"minecraft:consumable": {"on_consume_effects": [{"type": "minecraft:explode"}]}}"#,
            r#"{"minecraft:consumable": {"on_consume_effects": [{"type": "minecraft:apply_effects",
                "effects": [{"id": "minecraft:speed", "hidden_effect": {}}]}]}}"#,
            r#"{"minecraft:consumable": {"on_consume_effects": [{"type": "minecraft:apply_effects",
                "effects": [], "probability": 1.5}]}}"#,
            r#"{"minecraft:suspicious_stew_effects": [{"id": "minecraft:speed"}]}"#,
            r#"{"minecraft:food": {"nutrition": 1, "saturation": 1, "eat_seconds": 2}}"#,
        ] {
            assert!(read_json(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_potion_contents_that_holds_something_is_a_default() {
        let defaults = read_json(
            r#"{"minecraft:potion_contents": {"potion": "minecraft:swiftness", "custom_color": 255,
                "custom_effects": [{"id": "minecraft:speed", "duration": 20}]}}"#,
        )
        .unwrap();
        let registries = Registries {
            items: &[],
            mob_effects: &named(&["minecraft:speed"]),
            sound_events: &[],
        };
        let fields = render(&defaults, &registries).unwrap();
        assert_eq!(
            fields[0],
            (
                "potion_contents",
                "Some(PotionContents { potion: Some(\"minecraft:swiftness\"), custom_colour: Some(255), custom_effects: &[EffectInstance::new(\"minecraft:speed\", 20, 0)], custom_name: None })".to_owned()
            )
        );
        let alone = read_json(r#"{"minecraft:potion_contents": "minecraft:water"}"#).unwrap();
        assert_eq!(
            alone.potion_contents.unwrap().potion.as_deref(),
            Some("minecraft:water")
        );
    }

    #[test]
    fn the_rendering_names_items_by_id_and_checks_effects_and_sounds() {
        let defaults = read_json(FIXTURE).unwrap();
        let items = named(&["minecraft:air", "minecraft:glass_bottle"]);
        let effects = named(&["minecraft:hunger", "minecraft:poison"]);
        let sounds = named(&[
            "minecraft:item.honey_bottle.drink",
            "minecraft:item.ominous_bottle.dispose",
        ]);
        let registries = Registries {
            items: &items,
            mob_effects: &effects,
            sound_events: &sounds,
        };
        let fields = render(&defaults, &registries).unwrap();
        let names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            [
                "potion_contents",
                "potion_duration_scale",
                "damage",
                "use_cooldown",
                "fireworks_flight_duration",
                "charged_projectiles",
                "chicken_variant",
                "consumable",
                "food",
                "use_remainder",
                "ominous_bottle_amplifier",
            ]
        );
        let value = |name: &str| {
            fields
                .iter()
                .find(|(known, _)| *known == name)
                .unwrap()
                .1
                .clone()
        };
        assert_eq!(value("potion_duration_scale"), "Some(0.25)");
        assert_eq!(
            value("use_cooldown"),
            "Some(UseCooldown { seconds: 1.0, group: None })"
        );
        assert_eq!(
            value("food"),
            "Some(Food { nutrition: 3, saturation: 1.8000001, can_always_eat: true })"
        );
        assert_eq!(
            value("use_remainder"),
            "Some(UseRemainder { item: 1, count: 1 })"
        );
        assert_eq!(value("chicken_variant"), "Some(\"minecraft:cold\")");
        assert_eq!(value("charged_projectiles"), "true");
        assert_eq!(
            value("consumable"),
            "Some(Consumable { consume_seconds: 1.6, animation: UseAnimation::Drink, \
             sound: \"minecraft:item.honey_bottle.drink\", has_consume_particles: false, \
             on_consume_effects: &[ConsumeEffect::ApplyEffects { effects: \
             &[EffectInstance::new(\"minecraft:hunger\", 600, 0), EffectInstance { effect: \
             \"minecraft:poison\", duration: 100, amplifier: 1, ambient: false, visible: false, \
             show_icon: false }], probability: 0.3 }, \
             ConsumeEffect::RemoveEffects(EffectSet::Listed(&[\"minecraft:poison\"])), \
             ConsumeEffect::RemoveEffects(EffectSet::Tag(\"minecraft:bad\")), \
             ConsumeEffect::ClearAllEffects, ConsumeEffect::TeleportRandomly { diameter: 16.0, \
             directional_particles: true }, \
             ConsumeEffect::PlaySound(\"minecraft:item.ominous_bottle.dispose\")] })"
        );

        let unknown = Registries {
            items: &items,
            mob_effects: &named(&["minecraft:hunger"]),
            sound_events: &sounds,
        };
        assert!(render(&defaults, &unknown).is_err(), "poison is not known");
        let no_bottle = Registries {
            items: &named(&["minecraft:air"]),
            mob_effects: &effects,
            sound_events: &sounds,
        };
        assert!(render(&defaults, &no_bottle).is_err());
    }

    #[test]
    fn a_float_literal_reads_back_as_the_same_float() {
        for value in [1.8000001_f32, 7.2000003, 0.125, 1.6, 16.0, 0.3] {
            let literal = float_literal(value.to_bits());
            assert_eq!(literal.parse::<f32>().unwrap().to_bits(), value.to_bits());
            assert!(literal.contains('.'), "{literal}");
        }
    }
}
