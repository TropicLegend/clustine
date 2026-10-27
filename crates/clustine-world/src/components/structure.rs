//! Each component's structure as the game's `Codec` has it (`docs/groundwork/n9-plan.md`,
//! sections 3.1 to 3.3): a small tree of the primitive values the game's `DynamicOps`
//! tell apart, from which the containers' NBT and the menus' hashes are both made, so
//! that the game's codecs are read from the bytecode once and written here once.
//!
//! [`structure`] is the game's `encodeStart` of a component: optional fields are left
//! out at their defaults, registry values are namespaced names, and numbers have the
//! game's types (an effect's amplifier and a rocket's flight a byte, a duration an int, a
//! scale a float). [`component_from`] is its `parse`: fields are optional where the
//! codec's are, a field the codec reads leniently falls back to its default when it does
//! not read, and a number of any type reads as the type wanted, as the game's
//! `getNumberValue` does. Registry names are checked for their form only; whether a
//! potion or an effect exists is the edge's check, against the registries (3.1).

use std::fmt;

use super::{
    BannerLayer, Component, Components, EffectInstance, FireworkExplosion, FireworkShape,
    Fireworks, Kind, PotionContents, Rarity, TooltipDisplay, UseCooldown,
};
use crate::{Float, ItemStack};

/// A value as the game's `DynamicOps` see it. A map keeps its keys in the order the
/// codec writes them; the hash sorts them and NBT does not care, but a reader of the
/// JSON finds the game's order.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
    String(String),
    List(Vec<Value>),
    Map(Vec<(String, Value)>),
}

/// Why a structure does not read as a component: the game's codec would refuse it too,
/// or it holds what Clustine does not keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureError(pub String);

impl fmt::Display for StructureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StructureError {}

type Read<T> = Result<T, StructureError>;

fn refused<T>(message: impl Into<String>) -> Read<T> {
    Err(StructureError(message.into()))
}

/// The game's `Codec` encoding of `component`.
pub fn structure(component: &Component) -> Value {
    match component {
        Component::PotionContents(contents) => potion_contents(contents),
        Component::PotionDurationScale(Float(scale)) => Value::Float(*scale),
        Component::ChargedProjectiles(stacks) => {
            Value::List(stacks.iter().map(stack_structure).collect())
        }
        Component::Enchantments(levels) => Value::Map(
            levels
                .iter()
                .map(|(name, level)| (name.clone(), Value::Int(*level)))
                .collect(),
        ),
        Component::UseCooldown(cooldown) => {
            let mut map = Fields::default();
            map.put("seconds", Value::Float(cooldown.seconds.0));
            map.optional("cooldown_group", cooldown.group.as_deref().map(string));
            map.done()
        }
        Component::Fireworks(fireworks) => {
            let mut map = Fields::default();
            // `UNSIGNED_BYTE`: the game refuses to write a flight above 255; the world
            // keeps 0 to 3 by its rules, so the byte is the low eight bits.
            map.unless(
                fireworks.flight_duration != 0,
                "flight_duration",
                Value::Byte(fireworks.flight_duration as u8 as i8),
            );
            map.unless(
                !fireworks.explosions.is_empty(),
                "explosions",
                Value::List(fireworks.explosions.iter().map(explosion).collect()),
            );
            map.done()
        }
        Component::Damage(damage) => Value::Int(*damage),
        // `Unit.CODEC` writes an empty map.
        Component::IntangibleProjectile => Value::Map(Vec::new()),
        Component::BannerPatterns(layers) => Value::List(
            layers
                .iter()
                .map(|layer| {
                    Value::Map(vec![
                        ("pattern".to_owned(), string(&layer.pattern)),
                        ("color".to_owned(), string(&layer.colour)),
                    ])
                })
                .collect(),
        ),
        // The world holds only text that read (the edge and the store refuse other), so
        // the fall-back of a literal is never taken; it keeps the function total.
        Component::ItemName(json) => text_structure(json).unwrap_or_else(|_| string(json)),
        Component::Rarity(rarity) => string(rarity_name(*rarity)),
        Component::TooltipDisplay(display) => {
            let mut map = Fields::default();
            map.unless(display.hide_tooltip, "hide_tooltip", Value::Bool(true));
            map.unless(
                !display.hidden_components.is_empty(),
                "hidden_components",
                Value::List(
                    display
                        .hidden_components
                        .iter()
                        .map(String::as_str)
                        .map(string)
                        .collect(),
                ),
            );
            map.done()
        }
        Component::SuspiciousStewEffects(effects) => Value::List(
            effects
                .iter()
                .map(|(effect, duration)| {
                    let mut map = Fields::default();
                    map.put("id", string(effect));
                    map.unless(
                        *duration != STEW_DURATION,
                        "duration",
                        Value::Int(*duration),
                    );
                    map.done()
                })
                .collect(),
        ),
        Component::OminousBottleAmplifier(amplifier) => Value::Int(*amplifier),
        // A holder by name (`RegistryCodecs.holder`).
        Component::ChickenVariant(name) => string(name),
    }
}

/// The component of `kind` that `value` encodes, as the game's codec reads it.
pub fn component_from(kind: Kind, value: &Value) -> Read<Component> {
    let what = kind.name();
    Ok(match kind {
        Kind::PotionContents => Component::PotionContents(potion_contents_from(value)?),
        Kind::PotionDurationScale => {
            // `NON_NEGATIVE_FLOAT`.
            let scale = float(value, what)?;
            if scale.is_nan() || scale < 0.0 {
                return refused(format!("{what} must not be negative: {scale}"));
            }
            Component::PotionDurationScale(Float(scale))
        }
        Kind::ChargedProjectiles => {
            let stacks = list(value, what)?;
            if stacks.len() > MAX_CHARGED_PROJECTILES {
                return refused(format!(
                    "{what} has {} stacks, more than {MAX_CHARGED_PROJECTILES}",
                    stacks.len()
                ));
            }
            Component::ChargedProjectiles(stacks.iter().map(stack_from).collect::<Read<_>>()?)
        }
        Kind::Enchantments => {
            // `unboundedMap(Enchantment.CODEC, intRange(1, 255))`.
            let mut levels: Vec<(String, i32)> = Vec::new();
            for (name, level) in map(value, what)? {
                let name = identifier(name, what)?;
                let level = int_in(level, 1, 255, what)?;
                if levels.iter().any(|(known, _)| *known == name) {
                    return refused(format!("{what} names {name} twice"));
                }
                levels.push((name, level));
            }
            Component::Enchantments(super::in_registry_order(levels))
        }
        Kind::UseCooldown => {
            let fields = map(value, what)?;
            // `POSITIVE_FLOAT`.
            let seconds = float(required(fields, "seconds", what)?, what)?;
            if seconds.is_nan() || seconds <= 0.0 {
                return refused(format!("{what}'s seconds must be above 0: {seconds}"));
            }
            let group = optional(fields, "cooldown_group", |group| {
                identifier(text(group, what)?, what)
            })?;
            Component::UseCooldown(UseCooldown {
                seconds: Float(seconds),
                group,
            })
        }
        Kind::Fireworks => {
            let fields = map(value, what)?;
            let flight_duration =
                optional(fields, "flight_duration", |v| unsigned_byte(v, what))?.unwrap_or(0);
            let explosions = optional(fields, "explosions", |v| {
                let explosions = list(v, what)?;
                if explosions.len() > MAX_EXPLOSIONS {
                    return refused(format!(
                        "{what} has {} explosions, more than {MAX_EXPLOSIONS}",
                        explosions.len()
                    ));
                }
                explosions.iter().map(explosion_from).collect()
            })?
            .unwrap_or_default();
            Component::Fireworks(Fireworks {
                flight_duration,
                explosions,
            })
        }
        // `NON_NEGATIVE_INT`.
        Kind::Damage => Component::Damage(int_in(value, 0, i32::MAX, what)?),
        Kind::IntangibleProjectile => {
            map(value, what)?;
            Component::IntangibleProjectile
        }
        Kind::BannerPatterns => Component::BannerPatterns(
            list(value, what)?
                .iter()
                .map(|layer| {
                    let fields = map(layer, what)?;
                    let pattern =
                        identifier(text(required(fields, "pattern", what)?, what)?, what)?;
                    let colour = text(required(fields, "color", what)?, what)?;
                    if !DYE_COLOURS.contains(&colour) {
                        return refused(format!("{what} has no dye colour {colour:?}"));
                    }
                    Ok(BannerLayer {
                        pattern,
                        colour: colour.to_owned(),
                    })
                })
                .collect::<Read<_>>()?,
        ),
        Kind::ItemName => {
            let read = Text::from_value(value)?;
            Component::ItemName(json(&read.structure()))
        }
        Kind::Rarity => {
            let name = text(value, what)?;
            let rarity = [Rarity::Common, Rarity::Uncommon, Rarity::Rare, Rarity::Epic]
                .into_iter()
                .find(|rarity| rarity_name(*rarity) == name);
            match rarity {
                Some(rarity) => Component::Rarity(rarity),
                None => return refused(format!("{what} has no value {name:?}")),
            }
        }
        Kind::TooltipDisplay => {
            let fields = map(value, what)?;
            let hide_tooltip =
                optional(fields, "hide_tooltip", |v| boolean(v, what))?.unwrap_or(false);
            // A set: a kind named twice is kept once, where it was first named.
            let mut hidden_components: Vec<String> = Vec::new();
            for name in optional(fields, "hidden_components", |v| list(v, what))?.unwrap_or(&[]) {
                let name = identifier(text(name, what)?, what)?;
                if !hidden_components.contains(&name) {
                    hidden_components.push(name);
                }
            }
            Component::TooltipDisplay(TooltipDisplay {
                hide_tooltip,
                hidden_components,
            })
        }
        Kind::SuspiciousStewEffects => Component::SuspiciousStewEffects(
            list(value, what)?
                .iter()
                .map(|entry| {
                    let fields = map(entry, what)?;
                    let effect = identifier(text(required(fields, "id", what)?, what)?, what)?;
                    // `lenientOptionalFieldOf`: a duration that does not read is the
                    // default.
                    let duration = field(fields, "duration")
                        .and_then(|duration| int(duration, what).ok())
                        .unwrap_or(STEW_DURATION);
                    Ok((effect, duration))
                })
                .collect::<Read<_>>()?,
        ),
        Kind::OminousBottleAmplifier => {
            Component::OminousBottleAmplifier(int_in(value, 0, 4, what)?)
        }
        Kind::ChickenVariant => match value {
            Value::String(name) => Component::ChickenVariant(identifier(name, what)?),
            _ => {
                return refused(format!(
                    "{what} is not a variant's name (an inline variant is not kept)"
                ));
            }
        },
    })
}

/// The game's `ItemStackTemplate.CODEC` encoding of `stack`: its item by name, its count
/// left out at 1, and its components left out where it has none, an added one under its
/// kind's name and a removed kind under its name after `!`, with an empty map.
pub fn stack_structure(stack: &ItemStack) -> Value {
    let mut map = Fields::default();
    let item = usize::try_from(stack.item)
        .ok()
        .and_then(|id| clustine_data::ITEMS.get(id))
        .expect("a stack's item is an id in the item registry");
    map.put("id", string(item.name));
    map.unless(stack.count != 1, "count", Value::Int(stack.count));
    if let Some(patch) = &stack.components.0
        && !(patch.added.is_empty() && patch.removed.is_empty())
    {
        let mut entries: Vec<(String, Value)> = patch
            .added
            .iter()
            .map(|component| (component.kind().name().to_owned(), structure(component)))
            .collect();
        entries.extend(
            patch
                .removed
                .iter()
                .map(|kind| (format!("!{}", kind.name()), Value::Map(Vec::new()))),
        );
        map.put("components", Value::Map(entries));
    }
    map.done()
}

/// The stack that `value` encodes, as `ItemStackTemplate.CODEC` reads it: a map, or an
/// item's bare name for one of it.
pub fn stack_from(value: &Value) -> Read<ItemStack> {
    const WHAT: &str = "a stack";
    if let Value::String(name) = value {
        return Ok(ItemStack {
            item: item_named(name)?,
            count: 1,
            components: Components::NONE,
        });
    }
    let fields = map(value, WHAT)?;
    let item = item_named(text(required(fields, "id", WHAT)?, WHAT)?)?;
    let count = optional(fields, "count", |count| {
        int_in(count, 1, 99, "a stack's count")
    })?
    .unwrap_or(1);
    let mut components = Components::NONE;
    for (key, component) in optional(fields, "components", |v| map(v, WHAT))?.unwrap_or(&[]) {
        let (removed, name) = match key.strip_prefix('!') {
            Some(name) => (true, name),
            None => (false, key.as_str()),
        };
        let name = identifier(name, "a component's kind")?;
        let Some(kind) = Kind::named(&name) else {
            return refused(format!(
                "a stack has a component Clustine does not know: {name}"
            ));
        };
        // The patch keeps one entry a kind, the last read, as the game's map does.
        if removed {
            map(component, "a removed component")?;
            components.remove(kind);
        } else {
            components.set(component_from(kind, component)?);
        }
    }
    Ok(ItemStack {
        item,
        count,
        components,
    })
}

/// The game's `JsonOps` form of `value`, in its order, as compact JSON. A number keeps
/// only its value, as JSON does.
pub fn json(value: &Value) -> String {
    let mut out = String::new();
    write_json(value, &mut out);
    out
}

/// The duration a suspicious stew's effect has where its entry names none, in ticks.
const STEW_DURATION: i32 = 160;

/// `ChargedProjectiles.CODEC`'s limit.
const MAX_CHARGED_PROJECTILES: usize = 1024;

/// `Fireworks.CODEC`'s limit on explosions.
const MAX_EXPLOSIONS: usize = 256;

/// `DyeColor`'s names, in its order of ids.
const DYE_COLOURS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

fn string(text: &str) -> Value {
    Value::String(text.to_owned())
}

fn rarity_name(rarity: Rarity) -> &'static str {
    match rarity {
        Rarity::Common => "common",
        Rarity::Uncommon => "uncommon",
        Rarity::Rare => "rare",
        Rarity::Epic => "epic",
    }
}

fn shape_name(shape: FireworkShape) -> &'static str {
    match shape {
        FireworkShape::SmallBall => "small_ball",
        FireworkShape::LargeBall => "large_ball",
        FireworkShape::Star => "star",
        FireworkShape::Creeper => "creeper",
        FireworkShape::Burst => "burst",
    }
}

/// A map being written in the codec's order of fields.
#[derive(Default)]
struct Fields(Vec<(String, Value)>);

impl Fields {
    fn put(&mut self, name: &str, value: Value) {
        self.0.push((name.to_owned(), value));
    }

    /// `optionalFieldOf(name)`: written where present.
    fn optional(&mut self, name: &str, value: Option<Value>) {
        if let Some(value) = value {
            self.put(name, value);
        }
    }

    /// `optionalFieldOf(name, default)`: written where it differs from the default.
    fn unless(&mut self, differs: bool, name: &str, value: Value) {
        if differs {
            self.put(name, value);
        }
    }

    fn done(self) -> Value {
        Value::Map(self.0)
    }
}

fn potion_contents(contents: &PotionContents) -> Value {
    // `PotionContents.CODEC` writes with `FULL_CODEC`; the bare potion name is only read.
    let mut map = Fields::default();
    map.optional("potion", contents.potion.as_deref().map(string));
    map.optional("custom_color", contents.custom_colour.map(Value::Int));
    map.unless(
        !contents.custom_effects.is_empty(),
        "custom_effects",
        Value::List(contents.custom_effects.iter().map(effect).collect()),
    );
    map.optional("custom_name", contents.custom_name.as_deref().map(string));
    map.done()
}

/// `MobEffectInstance.CODEC`: the effect, then the fields of `Details.MAP_CODEC` in the
/// same map.
fn effect(effect: &EffectInstance) -> Value {
    let mut map = Fields::default();
    map.put("id", string(&effect.effect));
    // `UNSIGNED_BYTE`: the low eight bits, which is what the game keeps of an amplifier
    // it writes (it refuses one above 255; the world's are 0 to 255).
    map.unless(
        effect.amplifier != 0,
        "amplifier",
        Value::Byte(effect.amplifier as u8 as i8),
    );
    map.unless(
        effect.duration != 0,
        "duration",
        Value::Int(effect.duration),
    );
    map.unless(effect.ambient, "ambient", Value::Bool(true));
    map.unless(!effect.visible, "show_particles", Value::Bool(false));
    // Always written: `Optional.of(showIcon)`.
    map.put("show_icon", Value::Bool(effect.show_icon));
    map.done()
}

fn explosion(explosion: &FireworkExplosion) -> Value {
    let colours = |colours: &[i32]| Value::List(colours.iter().copied().map(Value::Int).collect());
    let mut map = Fields::default();
    map.put("shape", string(shape_name(explosion.shape)));
    map.unless(
        !explosion.colours.is_empty(),
        "colors",
        colours(&explosion.colours),
    );
    map.unless(
        !explosion.fade_colours.is_empty(),
        "fade_colors",
        colours(&explosion.fade_colours),
    );
    map.unless(explosion.trail, "has_trail", Value::Bool(true));
    map.unless(explosion.twinkle, "has_twinkle", Value::Bool(true));
    map.done()
}

fn potion_contents_from(value: &Value) -> Read<PotionContents> {
    const WHAT: &str = "minecraft:potion_contents";
    // `withAlternative(FULL_CODEC, Potion.CODEC)`: a bare potion's name reads too.
    if let Value::String(name) = value {
        return Ok(PotionContents {
            potion: Some(identifier(name, WHAT)?),
            ..PotionContents::default()
        });
    }
    let fields = map(value, WHAT)?;
    Ok(PotionContents {
        potion: optional(fields, "potion", |v| identifier(text(v, WHAT)?, WHAT))?,
        custom_colour: optional(fields, "custom_color", |v| int(v, WHAT))?,
        custom_effects: optional(fields, "custom_effects", |v| {
            list(v, WHAT)?.iter().map(effect_from).collect()
        })?
        .unwrap_or_default(),
        custom_name: optional(fields, "custom_name", |v| Ok(text(v, WHAT)?.to_owned()))?,
    })
}

fn effect_from(value: &Value) -> Read<EffectInstance> {
    const WHAT: &str = "an effect";
    let fields = map(value, WHAT)?;
    let effect = identifier(text(required(fields, "id", WHAT)?, WHAT)?, WHAT)?;
    if field(fields, "hidden_effect").is_some() {
        return refused("an effect with a hidden effect, which Clustine does not keep");
    }
    let visible = optional(fields, "show_particles", |v| boolean(v, WHAT))?.unwrap_or(true);
    Ok(EffectInstance {
        effect,
        amplifier: optional(fields, "amplifier", |v| unsigned_byte(v, WHAT))?.unwrap_or(0),
        duration: optional(fields, "duration", |v| int(v, WHAT))?.unwrap_or(0),
        ambient: optional(fields, "ambient", |v| boolean(v, WHAT))?.unwrap_or(false),
        visible,
        // `Details.create`: an icon shown where the particles are, unless it is named.
        show_icon: optional(fields, "show_icon", |v| boolean(v, WHAT))?.unwrap_or(visible),
    })
}

fn explosion_from(value: &Value) -> Read<FireworkExplosion> {
    const WHAT: &str = "a firework explosion";
    let fields = map(value, WHAT)?;
    let name = text(required(fields, "shape", WHAT)?, WHAT)?;
    let shape = [
        FireworkShape::SmallBall,
        FireworkShape::LargeBall,
        FireworkShape::Star,
        FireworkShape::Creeper,
        FireworkShape::Burst,
    ]
    .into_iter()
    .find(|shape| shape_name(*shape) == name);
    let Some(shape) = shape else {
        return refused(format!("{WHAT} has no shape {name:?}"));
    };
    let colours = |name: &str| -> Read<Vec<i32>> {
        Ok(optional(fields, name, |v| {
            list(v, WHAT)?
                .iter()
                .map(|colour| int(colour, WHAT))
                .collect()
        })?
        .unwrap_or_default())
    };
    Ok(FireworkExplosion {
        shape,
        colours: colours("colors")?,
        fade_colours: colours("fade_colors")?,
        trail: optional(fields, "has_trail", |v| boolean(v, WHAT))?.unwrap_or(false),
        twinkle: optional(fields, "has_twinkle", |v| boolean(v, WHAT))?.unwrap_or(false),
    })
}

fn item_named(name: &str) -> Read<i32> {
    let name = identifier(name, "a stack's item")?;
    // `Item.CODEC` refuses air: a template is never empty.
    if name == "minecraft:air" {
        return refused("a stack of air");
    }
    match clustine_data::item_id_by_name(&name) {
        Some(item) => Ok(item),
        None => refused(format!("no item is called {name}")),
    }
}

// The readers below are what the game's `DynamicOps` give a codec: a map, a list, a
// number of any type turned into the one wanted as Java's `Number` does, a boolean (a
// number other than 0, as NBT keeps them), a string.

fn map<'a>(value: &'a Value, what: &str) -> Read<&'a [(String, Value)]> {
    match value {
        Value::Map(entries) => Ok(entries),
        _ => refused(format!("{what} is not a map")),
    }
}

fn list<'a>(value: &'a Value, what: &str) -> Read<&'a [Value]> {
    match value {
        Value::List(items) => Ok(items),
        _ => refused(format!("{what} is not a list")),
    }
}

fn text<'a>(value: &'a Value, what: &str) -> Read<&'a str> {
    match value {
        Value::String(text) => Ok(text),
        _ => refused(format!("{what} has a value that is not a string")),
    }
}

fn field<'a>(fields: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn required<'a>(fields: &'a [(String, Value)], name: &str, what: &str) -> Read<&'a Value> {
    field(fields, name).ok_or_else(|| StructureError(format!("{what} has no {name}")))
}

/// `optionalFieldOf`: absent is `None`, present must read.
fn optional<'a, T>(
    fields: &'a [(String, Value)],
    name: &str,
    read: impl FnOnce(&'a Value) -> Read<T>,
) -> Read<Option<T>> {
    field(fields, name).map(read).transpose()
}

/// Java's `Number.intValue()` of a number: a long wraps, a float saturates.
fn int(value: &Value, what: &str) -> Read<i32> {
    Ok(match *value {
        Value::Byte(n) => i32::from(n),
        Value::Short(n) => i32::from(n),
        Value::Int(n) => n,
        Value::Long(n) => n as i32,
        Value::Float(n) => n as i32,
        Value::Double(n) => n as i32,
        _ => return refused(format!("{what} has a value that is not a number")),
    })
}

fn int_in(value: &Value, low: i32, high: i32, what: &str) -> Read<i32> {
    let n = int(value, what)?;
    if !(low..=high).contains(&n) {
        return refused(format!("{what} must be within {low} and {high}: {n}"));
    }
    Ok(n)
}

/// `UNSIGNED_BYTE`: `Codec.BYTE` (the number's low eight bits) read as 0 to 255.
fn unsigned_byte(value: &Value, what: &str) -> Read<i32> {
    // Java's `byteValue()` is the low eight bits of `intValue()` for every number.
    Ok(i32::from(int(value, what)? as u8))
}

fn float(value: &Value, what: &str) -> Read<f32> {
    Ok(match *value {
        Value::Float(n) => n,
        Value::Double(n) => n as f32,
        Value::Long(n) => n as f32,
        _ => int(value, what)? as f32,
    })
}

fn boolean(value: &Value, what: &str) -> Read<bool> {
    match value {
        Value::Bool(b) => Ok(*b),
        Value::Byte(_)
        | Value::Short(_)
        | Value::Int(_)
        | Value::Long(_)
        | Value::Float(_)
        | Value::Double(_) => Ok(int(value, what)? as i8 != 0),
        _ => refused(format!("{what} has a value that is not a boolean")),
    }
}

/// `Identifier.read` of a registry value's name: a name without a namespace is the
/// game's, and the characters are the game's few.
fn identifier(name: &str, what: &str) -> Read<String> {
    let (namespace, path) = match name.split_once(':') {
        Some(("", path)) => ("minecraft", path),
        Some((namespace, path)) => (namespace, path),
        None => ("minecraft", name),
    };
    let plain = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c);
    if !namespace.chars().all(plain) || !path.chars().all(|c| plain(c) || c == '/') {
        return refused(format!(
            "{what} has a name that is not an identifier: {name:?}"
        ));
    }
    Ok(format!("{namespace}:{path}"))
}

// Text components, as `ComponentSerialization.CODEC` has them, for what N9 meets: a
// literal or a translation, with the plain style fields and siblings. A text with what
// this does not read (a score, a selector, a key bind, NBT, an object, a click or hover
// event, a shadow colour, a translation's argument that is a number or a boolean) is
// refused rather than kept in part. The game ignores a key it does not know; this
// refuses it, since it cannot tell a key it does not know from one it does not keep.

/// The style fields read, in the order `Style.Serializer` writes them.
const STYLE_FIELDS: [(&str, StyleKind); 8] = [
    ("color", StyleKind::Text),
    ("bold", StyleKind::Flag),
    ("italic", StyleKind::Flag),
    ("underlined", StyleKind::Flag),
    ("strikethrough", StyleKind::Flag),
    ("obfuscated", StyleKind::Flag),
    ("insertion", StyleKind::Text),
    ("font", StyleKind::Text),
];

#[derive(Clone, Copy)]
enum StyleKind {
    Text,
    Flag,
}

struct Text {
    content: Content,
    style: Vec<(&'static str, Value)>,
    extra: Vec<Text>,
}

enum Content {
    Literal(String),
    Translatable {
        key: String,
        fallback: Option<String>,
        with: Vec<Text>,
    },
}

/// The structure of an item name the world keeps as the game's JSON.
fn text_structure(json: &str) -> Read<Value> {
    Ok(Text::from_value(&parse_json(json)?)?.structure())
}

impl Text {
    fn from_value(value: &Value) -> Read<Text> {
        const WHAT: &str = "a text component";
        match value {
            Value::String(literal) => Ok(Text {
                content: Content::Literal(literal.clone()),
                style: Vec::new(),
                extra: Vec::new(),
            }),
            // A list is its first text with the others appended to its siblings.
            Value::List(items) => {
                let Some((first, others)) = items.split_first() else {
                    return refused("an empty list of text components");
                };
                let mut text = Text::from_value(first)?;
                for other in others {
                    text.extra.push(Text::from_value(other)?);
                }
                Ok(text)
            }
            Value::Map(fields) => {
                for (key, _) in fields {
                    let known = matches!(
                        key.as_str(),
                        "type" | "text" | "translate" | "fallback" | "with" | "extra"
                    ) || STYLE_FIELDS.iter().any(|(name, _)| name == key);
                    if !known {
                        return refused(format!(
                            "{WHAT} with {key:?}, which Clustine does not read"
                        ));
                    }
                }
                let kind = optional(fields, "type", |v| text(v, WHAT))?;
                let content = match (kind, field(fields, "text"), field(fields, "translate")) {
                    (None | Some("text"), Some(literal), _) => {
                        Content::Literal(text(literal, WHAT)?.to_owned())
                    }
                    (None | Some("translatable"), _, Some(key)) => Content::Translatable {
                        key: text(key, WHAT)?.to_owned(),
                        fallback: optional(fields, "fallback", |v| Ok(text(v, WHAT)?.to_owned()))?,
                        with: optional(fields, "with", |v| {
                            list(v, WHAT)?.iter().map(Text::from_value).collect()
                        })?
                        .unwrap_or_default(),
                    },
                    _ => {
                        return refused(format!(
                            "{WHAT} that is neither a literal nor a translation"
                        ));
                    }
                };
                let mut style = Vec::new();
                for (name, kind) in STYLE_FIELDS {
                    if let Some(value) = field(fields, name) {
                        let value = match kind {
                            StyleKind::Text => Value::String(text(value, WHAT)?.to_owned()),
                            StyleKind::Flag => Value::Bool(boolean(value, WHAT)?),
                        };
                        style.push((name, value));
                    }
                }
                let extra = match field(fields, "extra") {
                    Some(extra) => {
                        let extra = list(extra, WHAT)?;
                        if extra.is_empty() {
                            return refused(format!("{WHAT} with an empty extra"));
                        }
                        extra.iter().map(Text::from_value).collect::<Read<_>>()?
                    }
                    None => Vec::new(),
                };
                Ok(Text {
                    content,
                    style,
                    extra,
                })
            }
            _ => refused(format!("{WHAT} that is neither a string, a list nor a map")),
        }
    }

    /// The game's encoding: a plain literal as a bare string (`tryCollapseToString`), any
    /// other as a map of its content, its siblings and its style.
    fn structure(&self) -> Value {
        if let Content::Literal(literal) = &self.content
            && self.style.is_empty()
            && self.extra.is_empty()
        {
            return string(literal);
        }
        let mut map = Fields::default();
        match &self.content {
            Content::Literal(literal) => map.put("text", string(literal)),
            Content::Translatable {
                key,
                fallback,
                with,
            } => {
                map.put("translate", string(key));
                map.optional("fallback", fallback.as_deref().map(string));
                map.unless(
                    !with.is_empty(),
                    "with",
                    Value::List(with.iter().map(Text::structure).collect()),
                );
            }
        }
        map.unless(
            !self.extra.is_empty(),
            "extra",
            Value::List(self.extra.iter().map(Text::structure).collect()),
        );
        for (name, value) in &self.style {
            map.put(name, value.clone());
        }
        map.done()
    }
}

/// A small reader of JSON into a value: objects, arrays, strings, booleans, and numbers
/// as doubles. A text component holds no numbers that this reads, and null nothing.
fn parse_json(json: &str) -> Read<Value> {
    let mut reader = JsonReader {
        chars: json.chars().collect(),
        at: 0,
    };
    let value = reader.value()?;
    reader.blank();
    if reader.at != reader.chars.len() {
        return refused("text after the JSON");
    }
    Ok(value)
}

struct JsonReader {
    chars: Vec<char>,
    at: usize,
}

impl JsonReader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Read<char> {
        let c = self
            .peek()
            .ok_or_else(|| StructureError("JSON ends early".into()))?;
        self.at += 1;
        Ok(c)
    }

    fn blank(&mut self) {
        while self.peek().is_some_and(|c| " \t\n\r".contains(c)) {
            self.at += 1;
        }
    }

    fn expect(&mut self, wanted: char) -> Read<()> {
        self.blank();
        match self.next()? {
            c if c == wanted => Ok(()),
            c => refused(format!("JSON has {c:?} where {wanted:?} belongs")),
        }
    }

    fn word(&mut self, word: &str, value: Value) -> Read<Value> {
        for wanted in word.chars() {
            if self.next()? != wanted {
                return refused("JSON has a word it should not");
            }
        }
        Ok(value)
    }

    fn value(&mut self) -> Read<Value> {
        self.blank();
        match self.peek() {
            Some('{') => {
                self.at += 1;
                let mut fields = Vec::new();
                self.blank();
                if self.peek() == Some('}') {
                    self.at += 1;
                    return Ok(Value::Map(fields));
                }
                loop {
                    self.blank();
                    let key = self.string()?;
                    self.expect(':')?;
                    fields.push((key, self.value()?));
                    self.blank();
                    match self.next()? {
                        ',' => {}
                        '}' => return Ok(Value::Map(fields)),
                        c => return refused(format!("JSON has {c:?} in an object")),
                    }
                }
            }
            Some('[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.blank();
                if self.peek() == Some(']') {
                    self.at += 1;
                    return Ok(Value::List(items));
                }
                loop {
                    items.push(self.value()?);
                    self.blank();
                    match self.next()? {
                        ',' => {}
                        ']' => return Ok(Value::List(items)),
                        c => return refused(format!("JSON has {c:?} in an array")),
                    }
                }
            }
            Some('"') => Ok(Value::String(self.string()?)),
            Some('t') => self.word("true", Value::Bool(true)),
            Some('f') => self.word("false", Value::Bool(false)),
            Some(c) if c == '-' || c.is_ascii_digit() => {
                let start = self.at;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || "+-.eE".contains(c))
                {
                    self.at += 1;
                }
                let number: String = self.chars[start..self.at].iter().collect();
                match number.parse::<f64>() {
                    Ok(number) => Ok(Value::Double(number)),
                    Err(_) => refused(format!("JSON has a number that is not one: {number}")),
                }
            }
            _ => refused("JSON has a value that is not one Clustine reads"),
        }
    }

    fn string(&mut self) -> Read<String> {
        if self.next()? != '"' {
            return refused("JSON has a key that is not a string");
        }
        let mut out = String::new();
        loop {
            match self.next()? {
                '"' => return Ok(out),
                '\\' => match self.next()? {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let high = self.hex()?;
                        let unit = if (0xD800..0xDC00).contains(&high) {
                            if self.next()? != '\\' || self.next()? != 'u' {
                                return refused("JSON has half a surrogate pair");
                            }
                            let low = self.hex()?;
                            if !(0xDC00..0xE000).contains(&low) {
                                return refused("JSON has half a surrogate pair");
                            }
                            0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                        } else {
                            high
                        };
                        match char::from_u32(unit) {
                            Some(c) => out.push(c),
                            None => return refused("JSON has half a surrogate pair"),
                        }
                    }
                    c => return refused(format!("JSON has an escape it should not: \\{c}")),
                },
                c => out.push(c),
            }
        }
    }

    fn hex(&mut self) -> Read<u32> {
        let mut unit = 0;
        for _ in 0..4 {
            let digit = self.next()?.to_digit(16);
            let Some(digit) = digit else {
                return refused("JSON has an escape that is not four hexadecimal digits");
            };
            unit = unit * 16 + digit;
        }
        Ok(unit)
    }
}

fn write_json(value: &Value, out: &mut String) {
    use std::fmt::Write;
    match value {
        Value::Empty => out.push_str("null"),
        Value::Byte(n) => write!(out, "{n}").expect("a string takes what is written"),
        Value::Short(n) => write!(out, "{n}").expect("a string takes what is written"),
        Value::Int(n) => write!(out, "{n}").expect("a string takes what is written"),
        Value::Long(n) => write!(out, "{n}").expect("a string takes what is written"),
        Value::Float(n) => write!(out, "{n:?}").expect("a string takes what is written"),
        Value::Double(n) => write!(out, "{n:?}").expect("a string takes what is written"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::String(text) => write_json_string(text, out),
        Value::List(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_json(item, out);
            }
            out.push(']');
        }
        Value::Map(entries) => {
            out.push('{');
            for (at, (key, item)) in entries.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                write_json_string(key, out);
                out.push(':');
                write_json(item, out);
            }
            out.push('}');
        }
    }
}

fn write_json_string(text: &str, out: &mut String) {
    use std::fmt::Write;
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(c)).expect("a string takes what is written");
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Value {
        string(text)
    }

    fn m(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
        )
    }

    fn item(name: &str) -> i32 {
        clustine_data::item_id_by_name(name).expect("the item exists")
    }

    fn effect(name: &str, amplifier: i32, duration: i32) -> EffectInstance {
        EffectInstance {
            effect: name.to_owned(),
            amplifier,
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
        }
    }

    /// One component or more of every kind, with fields at their defaults and not.
    fn samples() -> Vec<Component> {
        vec![
            Component::PotionContents(PotionContents::default()),
            Component::PotionContents(PotionContents {
                potion: Some("minecraft:swiftness".into()),
                ..PotionContents::default()
            }),
            Component::PotionContents(PotionContents {
                potion: Some("minecraft:water".into()),
                custom_colour: Some(0x00FF_0000),
                custom_effects: vec![
                    effect("minecraft:speed", 1, 200),
                    EffectInstance {
                        ambient: true,
                        visible: false,
                        show_icon: true,
                        ..effect("minecraft:poison", 0, 100)
                    },
                    EffectInstance {
                        show_icon: false,
                        ..effect("minecraft:strength", 255, -1)
                    },
                ],
                custom_name: Some("fancy".into()),
            }),
            Component::PotionDurationScale(Float(0.25)),
            Component::ChargedProjectiles(Vec::new()),
            Component::ChargedProjectiles(vec![
                ItemStack::new(item("minecraft:arrow"), 1),
                ItemStack::new(item("minecraft:firework_rocket"), 3),
            ]),
            Component::Enchantments(Vec::new()),
            Component::Enchantments(vec![
                ("minecraft:sharpness".into(), 3),
                ("minecraft:unbreaking".into(), 1),
            ]),
            Component::UseCooldown(UseCooldown {
                seconds: Float(1.0),
                group: None,
            }),
            Component::UseCooldown(UseCooldown {
                seconds: Float(2.5),
                group: Some("minecraft:ender_pearl".into()),
            }),
            Component::Fireworks(Fireworks::default()),
            Component::Fireworks(Fireworks {
                flight_duration: 2,
                explosions: vec![
                    FireworkExplosion {
                        shape: FireworkShape::Star,
                        colours: vec![0x00FF_0000],
                        fade_colours: vec![0xFF],
                        trail: true,
                        twinkle: false,
                    },
                    FireworkExplosion {
                        shape: FireworkShape::Creeper,
                        colours: Vec::new(),
                        fade_colours: Vec::new(),
                        trail: false,
                        twinkle: true,
                    },
                ],
            }),
            Component::Damage(0),
            Component::Damage(5),
            Component::IntangibleProjectile,
            Component::BannerPatterns(vec![
                BannerLayer {
                    pattern: "minecraft:stripe_bottom".into(),
                    colour: "red".into(),
                },
                BannerLayer {
                    pattern: "minecraft:cross".into(),
                    colour: "white".into(),
                },
            ]),
            Component::ItemName("\"Hi\"".into()),
            Component::ItemName(r#"{"translate":"block.minecraft.ominous_banner"}"#.into()),
            Component::ItemName(
                r#"{"text":"a","extra":["b"],"color":"gold","italic":false}"#.into(),
            ),
            Component::Rarity(Rarity::Common),
            Component::Rarity(Rarity::Epic),
            Component::TooltipDisplay(TooltipDisplay::default()),
            Component::TooltipDisplay(TooltipDisplay {
                hide_tooltip: true,
                hidden_components: vec!["minecraft:enchantments".into(), "minecraft:damage".into()],
            }),
            Component::SuspiciousStewEffects(vec![
                ("minecraft:night_vision".into(), 100),
                ("minecraft:saturation".into(), STEW_DURATION),
            ]),
            Component::OminousBottleAmplifier(4),
            Component::ChickenVariant("minecraft:cold".into()),
        ]
    }

    #[test]
    fn every_component_reads_back_from_its_structure() {
        for component in samples() {
            let value = structure(&component);
            assert_eq!(
                component_from(component.kind(), &value),
                Ok(component.clone()),
                "{value:?}"
            );
        }
    }

    #[test]
    fn fields_at_their_defaults_are_left_out() {
        assert_eq!(
            structure(&Component::PotionContents(PotionContents::default())),
            m(&[])
        );
        assert_eq!(
            structure(&Component::Fireworks(Fireworks::default())),
            m(&[])
        );
        assert_eq!(
            structure(&Component::TooltipDisplay(TooltipDisplay::default())),
            m(&[])
        );
        assert_eq!(
            structure(&Component::SuspiciousStewEffects(vec![(
                "minecraft:saturation".into(),
                STEW_DURATION
            )])),
            Value::List(vec![m(&[("id", s("minecraft:saturation"))])])
        );
    }

    #[test]
    fn an_effect_writes_its_amplifier_as_a_byte_and_its_icon_always() {
        let contents = PotionContents {
            custom_effects: vec![effect("minecraft:strength", 200, 20)],
            ..PotionContents::default()
        };
        assert_eq!(
            structure(&Component::PotionContents(contents)),
            m(&[(
                "custom_effects",
                Value::List(vec![m(&[
                    ("id", s("minecraft:strength")),
                    ("amplifier", Value::Byte(200u8 as i8)),
                    ("duration", Value::Int(20)),
                    ("show_icon", Value::Bool(true)),
                ])])
            )])
        );
    }

    #[test]
    fn a_bare_potion_name_reads_as_contents_of_that_potion() {
        assert_eq!(
            component_from(Kind::PotionContents, &s("swiftness")),
            Ok(Component::PotionContents(PotionContents {
                potion: Some("minecraft:swiftness".into()),
                ..PotionContents::default()
            }))
        );
    }

    #[test]
    fn an_effect_without_an_icon_named_shows_it_where_it_shows_particles() {
        let read = |show_particles: bool| {
            let value = m(&[(
                "custom_effects",
                Value::List(vec![m(&[
                    ("id", s("minecraft:speed")),
                    ("show_particles", Value::Bool(show_particles)),
                ])]),
            )]);
            match component_from(Kind::PotionContents, &value) {
                Ok(Component::PotionContents(contents)) => contents.custom_effects[0].show_icon,
                other => panic!("{other:?}"),
            }
        };
        assert!(read(true));
        assert!(!read(false));
    }

    #[test]
    fn numbers_and_booleans_read_whatever_their_type_as_nbt_keeps_them() {
        let value = m(&[
            ("flight_duration", Value::Int(3)),
            (
                "explosions",
                Value::List(vec![m(&[
                    ("shape", s("burst")),
                    ("colors", Value::List(vec![Value::Long(7)])),
                    ("has_trail", Value::Byte(1)),
                    ("has_twinkle", Value::Byte(0)),
                ])]),
            ),
        ]);
        assert_eq!(
            component_from(Kind::Fireworks, &value),
            Ok(Component::Fireworks(Fireworks {
                flight_duration: 3,
                explosions: vec![FireworkExplosion {
                    shape: FireworkShape::Burst,
                    colours: vec![7],
                    fade_colours: Vec::new(),
                    trail: true,
                    twinkle: false,
                }],
            }))
        );
        assert_eq!(
            component_from(Kind::PotionDurationScale, &Value::Double(0.5)),
            Ok(Component::PotionDurationScale(Float(0.5)))
        );
    }

    #[test]
    fn a_stews_duration_that_does_not_read_is_its_default() {
        let value = Value::List(vec![m(&[
            ("id", s("minecraft:night_vision")),
            ("duration", s("long")),
        ])]);
        assert_eq!(
            component_from(Kind::SuspiciousStewEffects, &value),
            Ok(Component::SuspiciousStewEffects(vec![(
                "minecraft:night_vision".into(),
                STEW_DURATION
            )]))
        );
    }

    #[test]
    fn a_stack_writes_its_count_only_above_one_and_reads_a_bare_name() {
        let arrow = item("minecraft:arrow");
        let one = ItemStack {
            item: arrow,
            count: 1,
            components: Components::NONE,
        };
        assert_eq!(stack_structure(&one), m(&[("id", s("minecraft:arrow"))]));
        assert_eq!(stack_from(&s("arrow")), Ok(one.clone()));
        let five = ItemStack { count: 5, ..one };
        assert_eq!(
            stack_structure(&five),
            m(&[("id", s("minecraft:arrow")), ("count", Value::Int(5))])
        );
        assert_eq!(stack_from(&stack_structure(&five)), Ok(five));
    }

    #[test]
    fn a_stack_with_a_removed_kind_writes_it_after_a_bang_and_reads_it_back() {
        let mut components = Components::NONE;
        components.set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:poison".into()),
            ..PotionContents::default()
        }));
        components.remove(Kind::PotionDurationScale);
        let stack = ItemStack {
            item: item("minecraft:tipped_arrow"),
            count: 1,
            components,
        };
        let value = stack_structure(&stack);
        assert_eq!(
            value,
            m(&[
                ("id", s("minecraft:tipped_arrow")),
                (
                    "components",
                    m(&[
                        (
                            "minecraft:potion_contents",
                            m(&[("potion", s("minecraft:poison"))])
                        ),
                        ("!minecraft:potion_duration_scale", m(&[])),
                    ])
                ),
            ])
        );
        assert_eq!(stack_from(&value), Ok(stack));
    }

    #[test]
    fn a_kind_both_added_and_removed_in_a_patch_is_what_was_read_last() {
        let value = m(&[
            ("id", s("minecraft:stick")),
            (
                "components",
                m(&[("minecraft:damage", Value::Int(2)), ("!damage", m(&[]))]),
            ),
        ]);
        let stack = stack_from(&value).expect("it reads");
        assert!(stack.components.removes(Kind::Damage));
        assert!(stack.components.get(Kind::Damage).is_none());
    }

    #[test]
    fn a_projectile_keeps_its_components() {
        let value = Value::List(vec![m(&[
            ("id", s("minecraft:tipped_arrow")),
            (
                "components",
                m(&[("minecraft:potion_contents", s("minecraft:poison"))]),
            ),
        ])]);
        let mut poison = Components::NONE;
        poison.set(Component::PotionContents(PotionContents {
            potion: Some("minecraft:poison".into()),
            ..PotionContents::default()
        }));
        let arrow = ItemStack {
            item: item("minecraft:tipped_arrow"),
            count: 1,
            components: poison,
        };
        assert_eq!(
            component_from(Kind::ChargedProjectiles, &value),
            Ok(Component::ChargedProjectiles(vec![arrow]))
        );
    }

    #[test]
    fn malformed_structures_are_refused() {
        let refused = [
            // Wrong types.
            (Kind::Damage, s("5")),
            (Kind::Rarity, Value::Int(1)),
            (Kind::Enchantments, Value::List(Vec::new())),
            (Kind::IntangibleProjectile, Value::Bool(true)),
            (Kind::PotionContents, m(&[("custom_color", s("red"))])),
            (Kind::TooltipDisplay, m(&[("hide_tooltip", s("yes"))])),
            // Required fields missing.
            (
                Kind::UseCooldown,
                m(&[("cooldown_group", s("minecraft:ender_pearl"))]),
            ),
            (
                Kind::PotionContents,
                m(&[(
                    "custom_effects",
                    Value::List(vec![m(&[("duration", Value::Int(1))])]),
                )]),
            ),
            (
                Kind::Fireworks,
                m(&[("explosions", Value::List(vec![m(&[])]))]),
            ),
            (
                Kind::BannerPatterns,
                Value::List(vec![m(&[("color", s("red"))])]),
            ),
            (Kind::SuspiciousStewEffects, Value::List(vec![m(&[])])),
            // Out of the codecs' ranges or names.
            (Kind::Damage, Value::Int(-1)),
            (Kind::OminousBottleAmplifier, Value::Int(5)),
            (
                Kind::Enchantments,
                m(&[("minecraft:sharpness", Value::Int(0))]),
            ),
            (Kind::UseCooldown, m(&[("seconds", Value::Float(0.0))])),
            (Kind::PotionDurationScale, Value::Float(-0.5)),
            (Kind::Rarity, s("legendary")),
            (
                Kind::BannerPatterns,
                Value::List(vec![m(&[
                    ("pattern", s("minecraft:cross")),
                    ("color", s("teal")),
                ])]),
            ),
            (Kind::PotionContents, s("Minecraft:Swiftness")),
            (Kind::ChickenVariant, m(&[])),
            // Text it does not read.
            (Kind::ItemName, Value::Int(3)),
            (Kind::ItemName, Value::List(Vec::new())),
            (
                Kind::ItemName,
                m(&[("text", s("a")), ("click_event", m(&[]))]),
            ),
            (Kind::ItemName, m(&[("score", m(&[]))])),
        ];
        for (kind, value) in refused {
            assert!(
                component_from(kind, &value).is_err(),
                "{kind:?} read {value:?}"
            );
        }
        for stack in [
            m(&[("count", Value::Int(2))]),
            m(&[("id", s("minecraft:air"))]),
            m(&[("id", s("minecraft:no_such_item"))]),
            m(&[("id", s("minecraft:arrow")), ("count", Value::Int(100))]),
            m(&[("id", s("minecraft:arrow")), ("count", Value::Int(0))]),
            m(&[
                ("id", s("minecraft:arrow")),
                (
                    "components",
                    m(&[("minecraft:lore", Value::List(Vec::new()))]),
                ),
            ]),
            m(&[
                ("id", s("minecraft:arrow")),
                ("components", m(&[("!minecraft:damage", Value::Int(0))])),
            ]),
        ] {
            assert!(stack_from(&stack).is_err(), "{stack:?}");
        }
    }

    #[test]
    fn a_plain_literal_is_a_bare_string_and_any_other_text_a_map() {
        let name = |json: &str| structure(&Component::ItemName(json.into()));
        assert_eq!(name("\"Hi\""), s("Hi"));
        assert_eq!(name(r#"{"text":"Hi"}"#), s("Hi"));
        assert_eq!(name(r#" { "type" : "text", "text" : "Hi" } "#), s("Hi"));
        assert_eq!(
            name(r#"{"translate":"item.minecraft.apple"}"#),
            m(&[("translate", s("item.minecraft.apple"))])
        );
        assert_eq!(
            name(r#"["a",{"text":"b","bold":true}]"#),
            m(&[
                ("text", s("a")),
                (
                    "extra",
                    Value::List(vec![m(&[("text", s("b")), ("bold", Value::Bool(true))])])
                ),
            ])
        );
        assert_eq!(
            name(r#"{"translate":"x","with":["é😀"],"fallback":"y"}"#),
            m(&[
                ("translate", s("x")),
                ("fallback", s("y")),
                ("with", Value::List(vec![s("\u{e9}\u{1F600}")])),
            ])
        );
    }

    #[test]
    fn a_name_read_from_a_structure_is_kept_as_the_json_of_that_structure() {
        assert_eq!(
            component_from(Kind::ItemName, &s("say \"hi\"")),
            Ok(Component::ItemName(r#""say \"hi\"""#.into()))
        );
        assert_eq!(
            component_from(
                Kind::ItemName,
                &m(&[("text", s("Hi")), ("bold", Value::Byte(1))])
            ),
            Ok(Component::ItemName(r#"{"text":"Hi","bold":true}"#.into()))
        );
    }

    #[test]
    fn a_name_without_a_namespace_is_the_games() {
        assert_eq!(
            component_from(Kind::ChickenVariant, &s("warm")),
            Ok(Component::ChickenVariant("minecraft:warm".into()))
        );
        assert_eq!(
            component_from(Kind::ChickenVariant, &s(":warm")),
            Ok(Component::ChickenVariant("minecraft:warm".into()))
        );
    }
}
