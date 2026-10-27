//! How an item stack is written wherever the store keeps one: in an entity, an effect,
//! a container's contents and a player's place (`docs/groundwork/n9-plan.md`, section
//! 3.2). All integers are big-endian.
//!
//! | Field | Type |
//! |---|---|
//! | Item, an id in the item registry | i32 |
//! | Count | i32 |
//! | Components added or changed: count, then each | u16 |
//! | Kinds removed: count, then each kind's name | u16 |
//!
//! A component is its kind's name in the game's `data_component_type` registry
//! (`minecraft:potion_contents`), the length in bytes of its value u32, and its value.
//! A name is a length u16 and that many bytes of UTF-8. A kind this server does not know
//! is skipped by its length, and a removed kind it does not know is passed over, so that
//! a later kind needs no new format: a server that knows it reads it, one that does not
//! reads the stack as if it were not there.
//!
//! Values are Clustine's own encoding. Registry values (a potion, an effect, an
//! enchantment, a pattern, a colour, a variant) are kept by name, as strings: a length
//! u32 and the UTF-8 bytes, so that a game update that renumbers a registry needs no new
//! format. A list is its count u32 and its elements; an option a flag u8 (0 none, 1
//! some) and, where some, the value; a flag u8 0 or 1; a float its `f32` bits. By kind:
//!
//! | Kind | Value |
//! |---|---|
//! | `potion_contents` | potion option of string, custom colour option of i32, custom effects list, custom name option of string |
//! | an effect of those | effect string, amplifier i32, duration i32, ambient flag, visible flag, show icon flag |
//! | `potion_duration_scale` | f32 |
//! | `charged_projectiles` | list of stacks, each written as above |
//! | `enchantments` | list of (enchantment string, level i32) |
//! | `use_cooldown` | seconds f32, group option of string |
//! | `fireworks` | flight duration i32, list of explosions |
//! | an explosion of those | shape u8 (the game's ids, 0 small ball to 4 burst), colours list of i32, fade colours list of i32, trail flag, twinkle flag |
//! | `damage` | i32 |
//! | `intangible_projectile` | nothing |
//! | `banner_patterns` | list of (pattern string, colour string) |
//! | `item_name` | string, the text component as the game's JSON writes it |
//! | `rarity` | u8 (the game's ids, 0 common to 3 epic) |
//! | `tooltip_display` | hide tooltip flag, hidden components list of string |
//! | `suspicious_stew_effects` | list of (effect string, duration i32) |
//! | `ominous_bottle_amplifier` | i32 |
//! | `chicken/variant` | string |
//!
//! A plain stack, which nearly every stack is, is its item and count and two counts of
//! 0: twelve bytes.

use clustine_world::components::{
    BannerLayer, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, Patch,
    PotionContents, Rarity, TooltipDisplay, UseCooldown,
};
use clustine_world::{Component, Components, Float, ItemStack, components::Kind};

use crate::FormatError;
use crate::bytes::Input;

/// How deep stacks may lie inside the components of stacks (a crossbow's projectiles
/// are stacks), so that a corrupt record cannot nest without end.
const MOST_NESTED: u8 = 8;

/// Writes `stack`, its components included.
pub(crate) fn put_stack(out: &mut Vec<u8>, stack: &ItemStack) {
    out.extend_from_slice(&stack.item.to_be_bytes());
    out.extend_from_slice(&stack.count.to_be_bytes());
    put_components(out, &stack.components);
}

/// Reads a stack [`put_stack`] wrote.
pub(crate) fn take_stack(input: &mut Input<'_>) -> Result<ItemStack, FormatError> {
    take_stack_within(input, 0)
}

/// u8 0 for none, else u8 1 and the stack.
pub(crate) fn put_optional_stack(out: &mut Vec<u8>, stack: Option<&ItemStack>) {
    match stack {
        None => out.push(0),
        Some(stack) => {
            out.push(1);
            put_stack(out, stack);
        }
    }
}

/// Reads a stack [`put_optional_stack`] wrote; `what` names its flag where it is wrong.
pub(crate) fn take_optional_stack(
    input: &mut Input<'_>,
    what: &'static str,
) -> Result<Option<ItemStack>, FormatError> {
    Ok(if take_flag(input, what)? {
        Some(take_stack(input)?)
    } else {
        None
    })
}

fn take_stack_within(input: &mut Input<'_>, depth: u8) -> Result<ItemStack, FormatError> {
    let item = input.i32()?;
    let count = input.i32()?;
    let components = take_components(input, depth)?;
    Ok(ItemStack {
        item,
        count,
        components,
    })
}

/// Writes a stack's components: those added or changed, each by its kind's name with
/// its length, then the kinds removed by name.
pub(crate) fn put_components(out: &mut Vec<u8>, components: &Components) {
    let empty = Patch::default();
    let patch = components.0.as_deref().unwrap_or(&empty);
    out.extend_from_slice(&count_u16(patch.added.len()).to_be_bytes());
    for component in &patch.added {
        put_name(out, component.kind().name());
        let mut value = Vec::new();
        put_value(&mut value, component);
        out.extend_from_slice(&length_u32(value.len()).to_be_bytes());
        out.extend_from_slice(&value);
    }
    out.extend_from_slice(&count_u16(patch.removed.len()).to_be_bytes());
    for kind in &patch.removed {
        put_name(out, kind.name());
    }
}

/// Reads what [`put_components`] wrote. A kind not known here is skipped; a kind named
/// twice, or both added and removed, is refused.
pub(crate) fn take_components(input: &mut Input<'_>, depth: u8) -> Result<Components, FormatError> {
    let mut components = Components::NONE;
    let mut seen: Vec<Kind> = Vec::new();
    for _ in 0..input.u16()? {
        let name = take_name(input)?;
        let length = usize::try_from(input.u32()?)
            .map_err(|_| FormatError::Corrupt("a component's length"))?;
        let bytes = input.take(length)?;
        let Some(kind) = Kind::named(name) else {
            continue;
        };
        if seen.contains(&kind) {
            return Err(FormatError::Corrupt("a component kind twice"));
        }
        seen.push(kind);
        let mut value = Input(bytes);
        let component = take_value(&mut value, kind, depth)?;
        value
            .finish()
            .map_err(|_| FormatError::Corrupt("a component longer than its value"))?;
        components.set(component);
    }
    for _ in 0..input.u16()? {
        let name = take_name(input)?;
        let Some(kind) = Kind::named(name) else {
            continue;
        };
        if seen.contains(&kind) {
            return Err(FormatError::Corrupt("a component kind twice"));
        }
        seen.push(kind);
        components.remove(kind);
    }
    Ok(components)
}

fn put_value(out: &mut Vec<u8>, component: &Component) {
    match component {
        Component::PotionContents(contents) => put_potion_contents(out, contents),
        Component::PotionDurationScale(scale) => put_float(out, *scale),
        Component::ChargedProjectiles(stacks) => put_list(out, stacks, put_stack),
        Component::Enchantments(levels) => put_list(out, levels, put_named_number),
        Component::UseCooldown(cooldown) => {
            put_float(out, cooldown.seconds);
            put_option(out, cooldown.group.as_ref(), |out, group| {
                put_string(out, group);
            });
        }
        Component::Fireworks(fireworks) => {
            out.extend_from_slice(&fireworks.flight_duration.to_be_bytes());
            put_list(out, &fireworks.explosions, put_explosion);
        }
        Component::Damage(damage) => out.extend_from_slice(&damage.to_be_bytes()),
        Component::IntangibleProjectile => {}
        Component::BannerPatterns(layers) => put_list(out, layers, |out, layer| {
            put_string(out, &layer.pattern);
            put_string(out, &layer.colour);
        }),
        Component::ItemName(name) => put_string(out, name),
        Component::Rarity(rarity) => out.push(match rarity {
            Rarity::Common => 0,
            Rarity::Uncommon => 1,
            Rarity::Rare => 2,
            Rarity::Epic => 3,
        }),
        Component::TooltipDisplay(display) => {
            out.push(u8::from(display.hide_tooltip));
            put_list(out, &display.hidden_components, |out, name| {
                put_string(out, name);
            });
        }
        Component::SuspiciousStewEffects(effects) => put_list(out, effects, put_named_number),
        Component::OminousBottleAmplifier(amplifier) => {
            out.extend_from_slice(&amplifier.to_be_bytes());
        }
        Component::ChickenVariant(variant) => put_string(out, variant),
    }
}

fn take_value(input: &mut Input<'_>, kind: Kind, depth: u8) -> Result<Component, FormatError> {
    Ok(match kind {
        Kind::PotionContents => Component::PotionContents(take_potion_contents(input)?),
        Kind::PotionDurationScale => Component::PotionDurationScale(Float(input.f32()?)),
        Kind::ChargedProjectiles => {
            if depth >= MOST_NESTED {
                return Err(FormatError::Corrupt("stacks nested too deep"));
            }
            Component::ChargedProjectiles(take_list(input, |input| {
                take_stack_within(input, depth + 1)
            })?)
        }
        Kind::Enchantments => Component::Enchantments(take_list(input, take_named_number)?),
        Kind::UseCooldown => Component::UseCooldown(UseCooldown {
            seconds: Float(input.f32()?),
            group: take_option(input, take_string)?,
        }),
        Kind::Fireworks => Component::Fireworks(Fireworks {
            flight_duration: input.i32()?,
            explosions: take_list(input, take_explosion)?,
        }),
        Kind::Damage => Component::Damage(input.i32()?),
        Kind::IntangibleProjectile => Component::IntangibleProjectile,
        Kind::BannerPatterns => Component::BannerPatterns(take_list(input, |input| {
            Ok(BannerLayer {
                pattern: take_string(input)?,
                colour: take_string(input)?,
            })
        })?),
        Kind::ItemName => Component::ItemName(take_string(input)?),
        Kind::Rarity => Component::Rarity(match input.u8()? {
            0 => Rarity::Common,
            1 => Rarity::Uncommon,
            2 => Rarity::Rare,
            3 => Rarity::Epic,
            _ => return Err(FormatError::Corrupt("a rarity")),
        }),
        Kind::TooltipDisplay => Component::TooltipDisplay(TooltipDisplay {
            hide_tooltip: take_flag(input, "a tooltip's hidden flag")?,
            hidden_components: take_list(input, take_string)?,
        }),
        Kind::SuspiciousStewEffects => {
            Component::SuspiciousStewEffects(take_list(input, take_named_number)?)
        }
        Kind::OminousBottleAmplifier => Component::OminousBottleAmplifier(input.i32()?),
        Kind::ChickenVariant => Component::ChickenVariant(take_string(input)?),
    })
}

/// Appends the value of a `potion_contents` component, which an area effect cloud keeps
/// too (`entities.rs`), so that the store has one encoding of it.
pub(crate) fn put_potion_contents(out: &mut Vec<u8>, contents: &PotionContents) {
    put_option(out, contents.potion.as_ref(), |out, potion| {
        put_string(out, potion);
    });
    put_option(out, contents.custom_colour.as_ref(), |out, colour| {
        out.extend_from_slice(&colour.to_be_bytes());
    });
    put_list(out, &contents.custom_effects, put_effect);
    put_option(out, contents.custom_name.as_ref(), |out, name| {
        put_string(out, name);
    });
}

/// Reads what [`put_potion_contents`] wrote.
pub(crate) fn take_potion_contents(input: &mut Input<'_>) -> Result<PotionContents, FormatError> {
    Ok(PotionContents {
        potion: take_option(input, take_string)?,
        custom_colour: take_option(input, |input| input.i32())?,
        custom_effects: take_list(input, take_effect)?,
        custom_name: take_option(input, take_string)?,
    })
}

fn put_effect(out: &mut Vec<u8>, effect: &EffectInstance) {
    put_string(out, &effect.effect);
    out.extend_from_slice(&effect.amplifier.to_be_bytes());
    out.extend_from_slice(&effect.duration.to_be_bytes());
    out.push(u8::from(effect.ambient));
    out.push(u8::from(effect.visible));
    out.push(u8::from(effect.show_icon));
}

fn take_effect(input: &mut Input<'_>) -> Result<EffectInstance, FormatError> {
    Ok(EffectInstance {
        effect: take_string(input)?,
        amplifier: input.i32()?,
        duration: input.i32()?,
        ambient: take_flag(input, "an effect's ambient flag")?,
        visible: take_flag(input, "an effect's visible flag")?,
        show_icon: take_flag(input, "an effect's icon flag")?,
    })
}

fn put_explosion(out: &mut Vec<u8>, explosion: &FireworkExplosion) {
    out.push(match explosion.shape {
        FireworkShape::SmallBall => 0,
        FireworkShape::LargeBall => 1,
        FireworkShape::Star => 2,
        FireworkShape::Creeper => 3,
        FireworkShape::Burst => 4,
    });
    let put_colour = |out: &mut Vec<u8>, colour: &i32| out.extend_from_slice(&colour.to_be_bytes());
    put_list(out, &explosion.colours, put_colour);
    put_list(out, &explosion.fade_colours, put_colour);
    out.push(u8::from(explosion.trail));
    out.push(u8::from(explosion.twinkle));
}

fn take_explosion(input: &mut Input<'_>) -> Result<FireworkExplosion, FormatError> {
    let shape = match input.u8()? {
        0 => FireworkShape::SmallBall,
        1 => FireworkShape::LargeBall,
        2 => FireworkShape::Star,
        3 => FireworkShape::Creeper,
        4 => FireworkShape::Burst,
        _ => return Err(FormatError::Corrupt("a firework's shape")),
    };
    Ok(FireworkExplosion {
        shape,
        colours: take_list(input, |input| input.i32())?,
        fade_colours: take_list(input, |input| input.i32())?,
        trail: take_flag(input, "a firework's trail flag")?,
        twinkle: take_flag(input, "a firework's twinkle flag")?,
    })
}

fn put_named_number(out: &mut Vec<u8>, (name, number): &(String, i32)) {
    put_string(out, name);
    out.extend_from_slice(&number.to_be_bytes());
}

fn take_named_number(input: &mut Input<'_>) -> Result<(String, i32), FormatError> {
    Ok((take_string(input)?, input.i32()?))
}

fn put_float(out: &mut Vec<u8>, value: Float) {
    out.extend_from_slice(&value.0.to_be_bytes());
}

fn put_option<T>(out: &mut Vec<u8>, value: Option<&T>, put: impl FnOnce(&mut Vec<u8>, &T)) {
    match value {
        None => out.push(0),
        Some(value) => {
            out.push(1);
            put(out, value);
        }
    }
}

fn take_option<T>(
    input: &mut Input<'_>,
    take: impl FnOnce(&mut Input<'_>) -> Result<T, FormatError>,
) -> Result<Option<T>, FormatError> {
    Ok(if take_flag(input, "an option's flag")? {
        Some(take(input)?)
    } else {
        None
    })
}

fn put_list<T>(out: &mut Vec<u8>, values: &[T], mut put: impl FnMut(&mut Vec<u8>, &T)) {
    out.extend_from_slice(&length_u32(values.len()).to_be_bytes());
    for value in values {
        put(out, value);
    }
}

fn take_list<T>(
    input: &mut Input<'_>,
    mut take: impl FnMut(&mut Input<'_>) -> Result<T, FormatError>,
) -> Result<Vec<T>, FormatError> {
    let count = input.u32()?;
    // Each element takes a byte at least, so a count beyond what is left is corrupt and
    // nothing is reserved for it.
    if usize::try_from(count).map_or(true, |count| count > input.0.len()) {
        return Err(FormatError::Corrupt("a list longer than its record"));
    }
    let mut values = Vec::new();
    for _ in 0..count {
        values.push(take(input)?);
    }
    Ok(values)
}

/// A string as every stack's strings are written: a length u32 and the UTF-8 bytes.
pub(crate) fn put_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&length_u32(value.len()).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

pub(crate) fn take_string(input: &mut Input<'_>) -> Result<String, FormatError> {
    let length =
        usize::try_from(input.u32()?).map_err(|_| FormatError::Corrupt("a string's length"))?;
    let bytes = input.take(length)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| FormatError::Corrupt("a string not UTF-8"))
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    out.extend_from_slice(&count_u16(name.len()).to_be_bytes());
    out.extend_from_slice(name.as_bytes());
}

fn take_name<'a>(input: &mut Input<'a>) -> Result<&'a str, FormatError> {
    let length = usize::from(input.u16()?);
    std::str::from_utf8(input.take(length)?)
        .map_err(|_| FormatError::Corrupt("a component's name not UTF-8"))
}

fn take_flag(input: &mut Input<'_>, what: &'static str) -> Result<bool, FormatError> {
    match input.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(FormatError::Corrupt(what)),
    }
}

fn count_u16(count: usize) -> u16 {
    u16::try_from(count).expect("a patch has fewer than 65536 components and names are short")
}

fn length_u32(length: usize) -> u32 {
    u32::try_from(length).expect("a component's value is shorter than 4 GiB")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stack with one component of every kind Clustine knows, a crossbow's projectile
    /// among them carrying components of its own.
    pub(crate) fn with_every_component() -> ItemStack {
        let mut arrow = ItemStack::new(800, 1);
        arrow
            .components
            .set(Component::PotionContents(PotionContents {
                potion: Some("minecraft:long_swiftness".into()),
                ..PotionContents::default()
            }));
        arrow.components.remove(Kind::Rarity);
        let mut stack = ItemStack::new(1_200, 3);
        let every = [
            Component::PotionContents(PotionContents {
                potion: Some("minecraft:strong_healing".into()),
                custom_colour: Some(0x00ff_8800),
                custom_effects: vec![EffectInstance {
                    effect: "minecraft:speed".into(),
                    amplifier: 1,
                    duration: -1,
                    ambient: true,
                    visible: false,
                    show_icon: true,
                }],
                custom_name: Some("ünicode".into()),
            }),
            Component::PotionDurationScale(Float(0.125)),
            Component::ChargedProjectiles(vec![arrow, ItemStack::new(801, 2)]),
            Component::Enchantments(vec![
                ("minecraft:quick_charge".into(), 3),
                ("minecraft:multishot".into(), 1),
            ]),
            Component::UseCooldown(UseCooldown {
                seconds: Float(1.5),
                group: Some("minecraft:ender_pearl".into()),
            }),
            Component::Fireworks(Fireworks {
                flight_duration: 2,
                explosions: vec![FireworkExplosion {
                    shape: FireworkShape::Creeper,
                    colours: vec![1, 0x00ff_ffff],
                    fade_colours: vec![],
                    trail: true,
                    twinkle: false,
                }],
            }),
            Component::Damage(17),
            Component::IntangibleProjectile,
            Component::BannerPatterns(vec![BannerLayer {
                pattern: "minecraft:rhombus".into(),
                colour: "cyan".into(),
            }]),
            Component::ItemName(r#"{"translate":"block.minecraft.ominous_banner"}"#.into()),
            Component::Rarity(Rarity::Uncommon),
            Component::TooltipDisplay(TooltipDisplay {
                hide_tooltip: false,
                hidden_components: vec!["minecraft:banner_patterns".into()],
            }),
            Component::SuspiciousStewEffects(vec![("minecraft:night_vision".into(), 100)]),
            Component::OminousBottleAmplifier(4),
            Component::ChickenVariant("minecraft:cold".into()),
        ];
        assert_eq!(every.len(), Kind::ALL.len(), "one component of every kind");
        for component in every {
            stack.components.set(component);
        }
        stack
    }

    fn round_trip(stack: &ItemStack) -> Vec<u8> {
        let mut out = Vec::new();
        put_stack(&mut out, stack);
        let mut input = Input(&out);
        assert_eq!(&take_stack(&mut input).unwrap(), stack);
        input.finish().unwrap();
        out
    }

    #[test]
    fn a_stack_with_one_component_of_every_kind_reads_as_it_was_written() {
        round_trip(&with_every_component());
    }

    #[test]
    fn a_plain_stack_is_its_item_and_count_and_two_counts_of_none() {
        let out = round_trip(&ItemStack::new(0x0102_0304, 64));
        assert_eq!(out, [1, 2, 3, 4, 0, 0, 0, 64, 0, 0, 0, 0]);
    }

    #[test]
    fn a_stack_that_removes_a_kind_its_item_has_by_default_keeps_the_removal() {
        let mut stack = ItemStack::new(5, 1);
        stack.components.remove(Kind::UseCooldown);
        stack.components.remove(Kind::ChickenVariant);
        round_trip(&stack);
    }

    /// A kind a later server writes is passed over by its length, and what follows it
    /// is read as written.
    #[test]
    fn a_component_of_a_kind_not_known_here_is_skipped_by_its_length() {
        let mut out = Vec::new();
        out.extend_from_slice(&7_i32.to_be_bytes());
        out.extend_from_slice(&1_i32.to_be_bytes());
        out.extend_from_slice(&2_u16.to_be_bytes());
        put_name(&mut out, "minecraft:a_later_kind");
        out.extend_from_slice(&3_u32.to_be_bytes());
        out.extend_from_slice(&[9, 9, 9]);
        put_name(&mut out, Kind::Damage.name());
        out.extend_from_slice(&4_u32.to_be_bytes());
        out.extend_from_slice(&5_i32.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        put_name(&mut out, "minecraft:another_later_kind");
        let mut input = Input(&out);
        let stack = take_stack(&mut input).unwrap();
        input.finish().unwrap();
        let mut expected = ItemStack::new(7, 1);
        expected.components.set(Component::Damage(5));
        assert_eq!(stack, expected);
    }

    #[test]
    fn a_component_whose_value_is_shorter_than_its_length_is_refused() {
        let mut out = Vec::new();
        out.extend_from_slice(&7_i32.to_be_bytes());
        out.extend_from_slice(&1_i32.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        put_name(&mut out, Kind::Damage.name());
        out.extend_from_slice(&5_u32.to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 5, 0]);
        out.extend_from_slice(&0_u16.to_be_bytes());
        assert!(matches!(
            take_stack(&mut Input(&out)),
            Err(FormatError::Corrupt(_))
        ));
    }

    #[test]
    fn a_kind_both_added_and_removed_is_refused() {
        let mut out = Vec::new();
        out.extend_from_slice(&7_i32.to_be_bytes());
        out.extend_from_slice(&1_i32.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        put_name(&mut out, Kind::IntangibleProjectile.name());
        out.extend_from_slice(&0_u32.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        put_name(&mut out, Kind::IntangibleProjectile.name());
        assert!(matches!(
            take_stack(&mut Input(&out)),
            Err(FormatError::Corrupt(_))
        ));
    }
}
