//! The conversion between a stack's components as the world keeps them and as the wire
//! carries them (`docs/groundwork/n9-plan.md`, sections 3.1 and 3.3), and the check of
//! what a client sends against the registries.
//!
//! The world keeps registry values by name (`clustine_world::components`); the wire
//! gives them as numbers (`clustine_protocol::components`). The numbers of the potion,
//! effect and component kind registries are fixed by the protocol (the generated
//! `NAMES`); those of the data-driven registries (enchantments, banner patterns,
//! chicken variants) are the order the edge sends them in at configuration
//! (`configuration::registry_packets`, from `clustine_data::SYNCED_REGISTRIES`), where an
//! entry's place in its list is its number.
//!
//! The protocol crate checks only the shape. What a client sends is checked here as the
//! game's codecs and registries would check it, and then once more by the world's own
//! reader of the component's structure (`component_from` over `structure`), so that the
//! edge takes nothing the store would refuse when it reads the stack back. Each limit
//! says which of the game's classes it is from (the dumps in `n9-javap`).

use std::fmt;

use clustine_data::{ITEMS, ItemComponents, Registry, item_id_by_name, synced_registry};
use clustine_protocol::components::{
    self as wire, BannerPattern, EffectDetails, WireComponent, WireComponents, WireStack,
    data_component_type, mob_effect, potion,
};
use clustine_protocol::nbt::Nbt;
use clustine_protocol::text::Text;
use clustine_world::components::{
    BannerLayer, EffectInstance, FireworkExplosion, FireworkShape, Fireworks, Kind, Patch,
    PotionContents, Rarity, TooltipDisplay, UseCooldown, Value, component_from, structure,
};
use clustine_world::{Component, Components, Float, ItemStack};

/// Why a client's components were refused: what the game's codec or a registry would
/// not take, or what the world does not keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

type Checked<T> = Result<T, Refusal>;

fn refused<T>(message: impl Into<String>) -> Checked<T> {
    Err(Refusal(message.into()))
}

/// `DyeColor`'s names, in its order of ids (white 0 to black 15), as the world keeps a
/// banner layer's colour.
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

/// The highest an effect's amplifier and a rocket's flight duration may be: the game's
/// codecs keep both as `ExtraCodecs.UNSIGNED_BYTE`, which refuses to write one above
/// 255 (n9 ExtraCodecs `lambda$static$23`), and the world keeps them so.
const MAX_UNSIGNED_BYTE: i32 = 255;

/// The highest count of a stack inside a component (n9 ItemStackTemplate, the count's
/// `ExtraCodecs.intRange(1, 99)`).
const MAX_TEMPLATE_COUNT: i32 = 99;

/// The world's patch as the wire carries it: the added components in the patch's order,
/// then the removed kinds by their ids.
pub(crate) fn wire_components(components: &Components) -> WireComponents {
    let Some(patch) = &components.0 else {
        return WireComponents::default();
    };
    wire_patch(patch)
}

fn wire_patch(patch: &Patch) -> WireComponents {
    WireComponents {
        added: patch.added.iter().filter_map(wire_component).collect(),
        removed: patch.removed.iter().map(|kind| kind_id(*kind)).collect(),
    }
}

/// The id of `kind` in the `data_component_type` registry.
fn kind_id(kind: Kind) -> i32 {
    // `WireComponent::IDS` lists the set in the order of `Kind`.
    WireComponent::IDS[kind as usize]
}

/// `component` as the wire has it; `None` where it holds a name no registry has, which
/// the world never keeps (it takes components only from here, from the store, which
/// reads only what this module or the world's rules wrote, and from the game's data).
fn wire_component(component: &Component) -> Option<WireComponent> {
    let converted = convert_to_wire(component);
    debug_assert!(
        converted.is_some(),
        "the world holds {component:?}, which names what no registry has: \
         every way into the world checks its names against the registries"
    );
    converted
}

fn convert_to_wire(component: &Component) -> Option<WireComponent> {
    Some(match component {
        Component::PotionContents(contents) => {
            WireComponent::PotionContents(wire::PotionContents {
                potion: match &contents.potion {
                    Some(name) => Some(position(&potion::NAMES, name)?),
                    None => None,
                },
                custom_colour: contents.custom_colour,
                custom_effects: contents
                    .custom_effects
                    .iter()
                    .map(|effect| {
                        Some(wire::EffectInstance {
                            effect: position(&mob_effect::NAMES, &effect.effect)?,
                            details: EffectDetails {
                                amplifier: effect.amplifier,
                                duration: effect.duration,
                                ambient: effect.ambient,
                                visible: effect.visible,
                                show_icon: effect.show_icon,
                                hidden: None,
                            },
                        })
                    })
                    .collect::<Option<_>>()?,
                custom_name: contents.custom_name.clone(),
            })
        }
        Component::PotionDurationScale(Float(scale)) => WireComponent::PotionDurationScale(*scale),
        Component::ChargedProjectiles(stacks) => WireComponent::ChargedProjectiles(
            stacks
                .iter()
                .map(|stack| WireStack {
                    item: stack.item,
                    count: stack.count,
                    components: wire_components(&stack.components),
                })
                .collect(),
        ),
        Component::Enchantments(levels) => WireComponent::Enchantments(wire_levels(levels)?),
        Component::StoredEnchantments(levels) => {
            WireComponent::StoredEnchantments(wire_levels(levels)?)
        }
        Component::UseCooldown(cooldown) => WireComponent::UseCooldown(wire::UseCooldown {
            seconds: cooldown.seconds.0,
            group: cooldown.group.clone(),
        }),
        Component::Fireworks(fireworks) => WireComponent::Fireworks(wire::Fireworks {
            flight_duration: fireworks.flight_duration,
            explosions: fireworks
                .explosions
                .iter()
                .map(|explosion| wire::FireworkExplosion {
                    shape: wire_shape(explosion.shape),
                    colours: explosion.colours.clone(),
                    fade_colours: explosion.fade_colours.clone(),
                    trail: explosion.trail,
                    twinkle: explosion.twinkle,
                })
                .collect(),
        }),
        Component::Damage(damage) => WireComponent::Damage(*damage),
        Component::IntangibleProjectile => WireComponent::IntangibleProjectile,
        Component::BannerPatterns(layers) => WireComponent::BannerPatterns(
            layers
                .iter()
                .map(|layer| {
                    Some(wire::BannerLayer {
                        pattern: BannerPattern::Registered(
                            banner_patterns().id_of(&layer.pattern)?,
                        ),
                        colour: position(&DYE_COLOURS, &layer.colour)?,
                    })
                })
                .collect::<Option<_>>()?,
        ),
        Component::ItemName(_) => {
            WireComponent::ItemName(Text::from_nbt(value_to_nbt(&structure(component))))
        }
        Component::Rarity(rarity) => WireComponent::Rarity(match rarity {
            Rarity::Common => wire::Rarity::Common,
            Rarity::Uncommon => wire::Rarity::Uncommon,
            Rarity::Rare => wire::Rarity::Rare,
            Rarity::Epic => wire::Rarity::Epic,
        }),
        Component::TooltipDisplay(display) => WireComponent::TooltipDisplay(wire::TooltipDisplay {
            hide_tooltip: display.hide_tooltip,
            hidden_components: display
                .hidden_components
                .iter()
                .map(|name| position(&data_component_type::NAMES, name))
                .collect::<Option<_>>()?,
        }),
        Component::SuspiciousStewEffects(effects) => WireComponent::SuspiciousStewEffects(
            effects
                .iter()
                .map(|(effect, duration)| Some((position(&mob_effect::NAMES, effect)?, *duration)))
                .collect::<Option<_>>()?,
        ),
        Component::OminousBottleAmplifier(amplifier) => {
            WireComponent::OminousBottleAmplifier(*amplifier)
        }
        Component::ChickenVariant(name) => {
            WireComponent::ChickenVariant(chicken_variants().id_of(name)?)
        }
        Component::DyedColor(rgb) => WireComponent::DyedColor(*rgb),
    })
}

/// Enchantments by name as the wire numbers them.
fn wire_levels(levels: &[(String, i32)]) -> Option<Vec<(i32, i32)>> {
    levels
        .iter()
        .map(|(name, level)| Some((enchantments().id_of(name)?, *level)))
        .collect()
}

/// The wire's patch as the world keeps it, checked as the game's codecs and registries
/// would check it. A kind added twice is the one read last, as the game's map keeps it
/// (n9 DataComponentPatch$3, `Reference2ObjectArrayMap.put`); a kind both added and
/// removed is removed, since the game puts the removals into the same map after the
/// additions; a removed kind outside Clustine's set is let go, as the world has nothing
/// of it to remove.
pub(crate) fn world_components(components: &WireComponents) -> Checked<Components> {
    let mut world = Components::NONE;
    for component in &components.added {
        world.set(world_component(component)?);
    }
    for id in &components.removed {
        if let Some(at) = WireComponent::IDS.iter().position(|known| known == id) {
            world.remove(Kind::ALL[at]);
        }
    }
    Ok(world)
}

/// `component` as the world keeps it, checked: first against the registries and the
/// limits the world's structure cannot show, then by the world's reader of what the
/// component's structure is, whose answer is kept (an identifier with its namespace
/// filled in, a list's repeats as the game keeps them).
fn world_component(component: &WireComponent) -> Checked<Component> {
    let converted = convert_to_world(component)?;
    let kind = converted.kind();
    component_from(kind, &structure(&converted))
        .map_err(|error| Refusal(format!("{}: {error}", kind.name())))
}

fn convert_to_world(component: &WireComponent) -> Checked<Component> {
    Ok(match component {
        WireComponent::PotionContents(contents) => Component::PotionContents(PotionContents {
            potion: match contents.potion {
                Some(id) => Some(named(&potion::NAMES, id, "potion")?.to_owned()),
                None => None,
            },
            custom_colour: contents.custom_colour,
            custom_effects: contents
                .custom_effects
                .iter()
                .map(world_effect)
                .collect::<Checked<_>>()?,
            custom_name: contents.custom_name.clone(),
        }),
        // `ExtraCodecs.NON_NEGATIVE_FLOAT` (n9 DataComponents, `lambda$static$54`); the
        // world's reader refuses a negative one too, and this says so first.
        WireComponent::PotionDurationScale(scale) => {
            if scale.is_nan() || *scale < 0.0 {
                return refused(format!("a potion duration scale below 0: {scale}"));
            }
            Component::PotionDurationScale(Float(*scale))
        }
        WireComponent::ChargedProjectiles(stacks) => {
            Component::ChargedProjectiles(stacks.iter().map(world_stack).collect::<Checked<_>>()?)
        }
        WireComponent::Enchantments(levels) => Component::Enchantments(world_levels(levels)?),
        // The same codecs as `enchantments` (n9 DataComponents, `lambda$static$45`).
        WireComponent::StoredEnchantments(levels) => {
            Component::StoredEnchantments(world_levels(levels)?)
        }
        WireComponent::UseCooldown(cooldown) => {
            // `ExtraCodecs.POSITIVE_FLOAT` (n9 UseCooldown); the group's form is the
            // protocol's check (`Identifier.STREAM_CODEC`) and the world's reader's.
            if cooldown.seconds.is_nan() || cooldown.seconds <= 0.0 {
                return refused(format!(
                    "a use cooldown of {} seconds, not above 0",
                    cooldown.seconds
                ));
            }
            Component::UseCooldown(UseCooldown {
                seconds: Float(cooldown.seconds),
                group: cooldown.group.clone(),
            })
        }
        WireComponent::Fireworks(fireworks) => {
            // `ExtraCodecs.UNSIGNED_BYTE` (n9 Fireworks); the stream codec reads a
            // `VAR_INT`, which the persistent codec could not write back.
            if !(0..=MAX_UNSIGNED_BYTE).contains(&fireworks.flight_duration) {
                return refused(format!(
                    "a rocket's flight duration of {}, outside 0 to {MAX_UNSIGNED_BYTE}",
                    fireworks.flight_duration
                ));
            }
            Component::Fireworks(Fireworks {
                flight_duration: fireworks.flight_duration,
                explosions: fireworks
                    .explosions
                    .iter()
                    .map(|explosion| FireworkExplosion {
                        shape: world_shape(explosion.shape),
                        colours: explosion.colours.clone(),
                        fade_colours: explosion.fade_colours.clone(),
                        trail: explosion.trail,
                        twinkle: explosion.twinkle,
                    })
                    .collect(),
            })
        }
        // `ExtraCodecs.NON_NEGATIVE_INT` (n9 DataComponents, `lambda$static$3`); the
        // stream codec is a bare `VAR_INT`.
        WireComponent::Damage(damage) => {
            if *damage < 0 {
                return refused(format!("damage below 0: {damage}"));
            }
            Component::Damage(*damage)
        }
        WireComponent::IntangibleProjectile => Component::IntangibleProjectile,
        WireComponent::BannerPatterns(layers) => Component::BannerPatterns(
            layers
                .iter()
                .map(|layer| {
                    let pattern = match &layer.pattern {
                        BannerPattern::Registered(id) => {
                            registry_name(banner_patterns(), *id, "banner pattern")?
                        }
                        // The game takes a pattern given whole (n9 BannerPattern's
                        // `holder` stream codec); the world keeps patterns by name only.
                        BannerPattern::Direct { asset_id, .. } => {
                            return refused(format!(
                                "a banner pattern given whole ({asset_id}), \
                                 which the world does not keep: it keeps patterns by name"
                            ));
                        }
                    };
                    // `DyeColor.STREAM_CODEC` reads one out of range as white
                    // (`ByIdMap.OutOfBoundsStrategy.ZERO`), as the protocol does; one
                    // made here out of range is refused rather than coloured white.
                    let colour = named(&DYE_COLOURS, layer.colour, "dye colour")?;
                    Ok(BannerLayer {
                        pattern,
                        colour: colour.to_owned(),
                    })
                })
                .collect::<Checked<_>>()?,
        ),
        WireComponent::ItemName(text) => component_from(Kind::ItemName, &nbt_to_value(text))
            .map_err(|error| Refusal(format!("an item name the world does not read: {error}")))?,
        WireComponent::Rarity(rarity) => Component::Rarity(match rarity {
            wire::Rarity::Common => Rarity::Common,
            wire::Rarity::Uncommon => Rarity::Uncommon,
            wire::Rarity::Rare => Rarity::Rare,
            wire::Rarity::Epic => Rarity::Epic,
        }),
        WireComponent::TooltipDisplay(display) => Component::TooltipDisplay(TooltipDisplay {
            hide_tooltip: display.hide_tooltip,
            // Kept by name, any kind of the registry (n9 TooltipDisplay,
            // `DataComponentType.STREAM_CODEC`, `ByteBufCodecs.registry`).
            hidden_components: display
                .hidden_components
                .iter()
                .map(|id| Ok(named(&data_component_type::NAMES, *id, "component kind")?.to_owned()))
                .collect::<Checked<_>>()?,
        }),
        WireComponent::SuspiciousStewEffects(effects) => Component::SuspiciousStewEffects(
            effects
                .iter()
                .map(|(effect, duration)| {
                    Ok((
                        named(&mob_effect::NAMES, *effect, "effect")?.to_owned(),
                        *duration,
                    ))
                })
                .collect::<Checked<_>>()?,
        ),
        // `ExtraCodecs.intRange(0, 4)` (n9 OminousBottleAmplifier); the stream codec is
        // a bare `VAR_INT`.
        WireComponent::OminousBottleAmplifier(amplifier) => {
            if !(0..=4).contains(amplifier) {
                return refused(format!(
                    "an ominous bottle's amplifier of {amplifier}, outside 0 to 4"
                ));
            }
            Component::OminousBottleAmplifier(*amplifier)
        }
        WireComponent::ChickenVariant(id) => {
            Component::ChickenVariant(registry_name(chicken_variants(), *id, "chicken variant")?)
        }
        // Any `int` (n10 DyedItemColor: `ByteBufCodecs.INT`, and `RGB_COLOR_CODEC`'s
        // `Codec.INT` writes any back).
        WireComponent::DyedColor(rgb) => Component::DyedColor(*rgb),
    })
}

/// `ItemEnchantments` as the world keeps it, checked.
fn world_levels(levels: &[(i32, i32)]) -> Checked<Vec<(String, i32)>> {
    // The game's map keeps an enchantment once, at the level given last
    // (n9 ItemEnchantments, `ByteBufCodecs.map`).
    let mut kept: Vec<(String, i32)> = Vec::with_capacity(levels.len());
    for (id, level) in levels {
        let name = registry_name(enchantments(), *id, "enchantment")?;
        // `LEVEL_CODEC`, `Codec.intRange(1, 255)` (n9 ItemEnchantments). The
        // stream codec's own check (its constructor) lets a level of 0 through,
        // which the persistent codec, and so the world, would not read back.
        if !(1..=wire::MAX_ENCHANTMENT_LEVEL).contains(level) {
            return refused(format!(
                "{name} at level {level}, outside 1 to {}",
                wire::MAX_ENCHANTMENT_LEVEL
            ));
        }
        match kept.iter_mut().find(|(known, _)| *known == name) {
            Some(entry) => entry.1 = *level,
            None => kept.push((name, *level)),
        }
    }
    Ok(clustine_world::components::in_registry_order(kept))
}

/// `MobEffectInstance` as the world keeps it.
fn world_effect(effect: &wire::EffectInstance) -> Checked<EffectInstance> {
    let name = named(&mob_effect::NAMES, effect.effect, "effect")?;
    let details = &effect.details;
    // The game keeps a weaker instance under a stronger one (n9 MobEffectInstance$Details,
    // `hidden_effect`); the world leaves it out (`clustine_world::components`), so one
    // given is refused rather than dropped unsaid.
    if details.hidden.is_some() {
        return refused(format!(
            "{name} with a hidden effect under it, which the world does not keep"
        ));
    }
    // `amplifier` is `ExtraCodecs.UNSIGNED_BYTE` (n9 MobEffectInstance$Details), which
    // refuses one above 255 and reads a negative one back as another; the stream codec
    // reads any `VAR_INT`. The duration is `Codec.INT`, any number (-1 without end).
    if !(0..=MAX_UNSIGNED_BYTE).contains(&details.amplifier) {
        return refused(format!(
            "{name} with amplifier {}, outside 0 to {MAX_UNSIGNED_BYTE}",
            details.amplifier
        ));
    }
    Ok(EffectInstance {
        effect: name.to_owned(),
        amplifier: details.amplifier,
        duration: details.duration,
        ambient: details.ambient,
        visible: details.visible,
        show_icon: details.show_icon,
    })
}

/// A stack inside a component (`ItemStackTemplate`) as the world keeps it.
fn world_stack(stack: &WireStack) -> Checked<ItemStack> {
    // `Item.STREAM_CODEC` refuses an id outside the registry and the template air.
    let Some(item) = usize::try_from(stack.item)
        .ok()
        .and_then(|id| ITEMS.get(id))
    else {
        return refused(format!("a stack of item {}, which is none", stack.item));
    };
    if Some(stack.item) == item_id_by_name("minecraft:air") {
        return refused("a stack of air inside a component");
    }
    // The template's count is `intRange(1, 99)` (n9 ItemStackTemplate); a client's stack
    // is also refused where a stack it holds is larger than its item stacks
    // (n9 ItemStack `validateContainedItemSizes`, for `charged_projectiles`).
    if !(1..=MAX_TEMPLATE_COUNT).contains(&stack.count) {
        return refused(format!(
            "a stack of {} {}, outside 1 to {MAX_TEMPLATE_COUNT}",
            stack.count, item.name
        ));
    }
    let max = i32::from(ItemComponents::of(stack.item).max_stack_size);
    if stack.count > max {
        return refused(format!(
            "a stack of {} {}, more than its {max} to a stack",
            stack.count, item.name
        ));
    }
    Ok(ItemStack {
        item: stack.item,
        count: stack.count,
        components: world_components(&stack.components)?,
    })
}

/// The edge's data-driven registries, in the order it sends them.
fn synced(name: &str) -> &'static Registry {
    synced_registry(name).expect("the edge sends every registry a component names")
}

fn enchantments() -> &'static Registry {
    synced("minecraft:enchantment")
}

fn banner_patterns() -> &'static Registry {
    synced("minecraft:banner_pattern")
}

fn chicken_variants() -> &'static Registry {
    synced("minecraft:chicken_variant")
}

/// The entry of number `id` in a registry the edge sent.
fn registry_name(registry: &Registry, id: i32, what: &str) -> Checked<String> {
    Ok(named(registry.entries, id, what)?.to_owned())
}

/// The entry of number `id` in `names`, refused where there is none.
fn named<'a>(names: &[&'a str], id: i32, what: &str) -> Checked<&'a str> {
    match usize::try_from(id).ok().and_then(|at| names.get(at)) {
        Some(name) => Ok(name),
        None => refused(format!("{what} {id}, which the registry does not have")),
    }
}

/// The number of `name` in `names`.
fn position(names: &[&str], name: &str) -> Option<i32> {
    let at = names.iter().position(|known| *known == name)?;
    i32::try_from(at).ok()
}

fn wire_shape(shape: FireworkShape) -> wire::FireworkShape {
    match shape {
        FireworkShape::SmallBall => wire::FireworkShape::SmallBall,
        FireworkShape::LargeBall => wire::FireworkShape::LargeBall,
        FireworkShape::Star => wire::FireworkShape::Star,
        FireworkShape::Creeper => wire::FireworkShape::Creeper,
        FireworkShape::Burst => wire::FireworkShape::Burst,
    }
}

fn world_shape(shape: wire::FireworkShape) -> FireworkShape {
    match shape {
        wire::FireworkShape::SmallBall => FireworkShape::SmallBall,
        wire::FireworkShape::LargeBall => FireworkShape::LargeBall,
        wire::FireworkShape::Star => FireworkShape::Star,
        wire::FireworkShape::Creeper => FireworkShape::Creeper,
        wire::FireworkShape::Burst => FireworkShape::Burst,
    }
}

// A text component's structure as NBT, as the game's `NbtOps` writes it: a boolean a
// byte, a map a compound, and a list a list tag. A list tag holds one type of tag, so
// one whose items differ (a text's siblings, some plain strings and some compounds) is
// written as compounds, each item that is not a compound, or that is a compound of the
// one key "" only, wrapped in a compound of the key "" (the game's `ListTag`
// `wrapIfNeeded` since 1.21.5); a reader unwraps them.

/// The key of a wrapped item of a list of mixed tags.
const WRAPPER: &str = "";

fn value_to_nbt(value: &Value) -> Nbt {
    match value {
        // `NbtOps.empty()` is the end tag, which no text holds; an empty compound keeps
        // this total.
        Value::Empty => Nbt::Compound(Vec::new()),
        Value::Byte(n) => Nbt::Byte(*n),
        Value::Short(n) => Nbt::Short(*n),
        Value::Int(n) => Nbt::Int(*n),
        Value::Long(n) => Nbt::Long(*n),
        Value::Float(n) => Nbt::Float(*n),
        Value::Double(n) => Nbt::Double(*n),
        Value::Bool(b) => Nbt::Byte(i8::from(*b)),
        Value::String(text) => Nbt::String(text.clone()),
        Value::List(items) => {
            let items: Vec<Nbt> = items.iter().map(value_to_nbt).collect();
            let mixed = items
                .split_first()
                .is_some_and(|(first, rest)| rest.iter().any(|item| !same_tag(first, item)));
            let compounds = items
                .first()
                .is_some_and(|first| matches!(first, Nbt::Compound(_)));
            if mixed || compounds {
                Nbt::List(items.into_iter().map(wrap_if_needed).collect())
            } else {
                Nbt::List(items)
            }
        }
        Value::Map(entries) => Nbt::Compound(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), value_to_nbt(value)))
                .collect(),
        ),
    }
}

fn same_tag(one: &Nbt, other: &Nbt) -> bool {
    std::mem::discriminant(one) == std::mem::discriminant(other)
}

fn is_wrapper(entries: &[(String, Nbt)]) -> bool {
    matches!(entries, [(key, _)] if key == WRAPPER)
}

fn wrap_if_needed(item: Nbt) -> Nbt {
    match item {
        Nbt::Compound(entries) if !is_wrapper(&entries) => Nbt::Compound(entries),
        other => Nbt::Compound(vec![(WRAPPER.to_owned(), other)]),
    }
}

/// A text component's NBT as the value its codec reads, as the game's `NbtOps` gives it:
/// a list's wrapped items unwrapped, and the arrays as lists of their numbers.
fn nbt_to_value(text: &Text) -> Value {
    tag_value(&text.to_nbt())
}

fn tag_value(tag: &Nbt) -> Value {
    match tag {
        Nbt::Byte(n) => Value::Byte(*n),
        Nbt::Short(n) => Value::Short(*n),
        Nbt::Int(n) => Value::Int(*n),
        Nbt::Long(n) => Value::Long(*n),
        Nbt::Float(n) => Value::Float(*n),
        Nbt::Double(n) => Value::Double(*n),
        Nbt::ByteArray(bytes) => Value::List(
            bytes
                .iter()
                .map(|byte| Value::Byte(i8::from_le_bytes([*byte])))
                .collect(),
        ),
        Nbt::String(text) => Value::String(text.clone()),
        Nbt::List(items) => Value::List(
            items
                .iter()
                .map(|item| match item {
                    Nbt::Compound(entries) if is_wrapper(entries) => tag_value(&entries[0].1),
                    other => tag_value(other),
                })
                .collect(),
        ),
        Nbt::Compound(entries) => Value::Map(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), tag_value(value)))
                .collect(),
        ),
        Nbt::IntArray(values) => Value::List(values.iter().copied().map(Value::Int).collect()),
        Nbt::LongArray(values) => Value::List(values.iter().copied().map(Value::Long).collect()),
    }
}

/// A client's stack as the world keeps it. Components the edge refuses (a number no
/// registry has, a value out of the game's range) are dropped all together, as the
/// stack's patch would not have read in the game (`docs/groundwork/n9-plan.md`,
/// section 3.3), and the item is kept with its defaults.
pub(crate) fn client_stack(
    stack: &clustine_protocol::item::ItemStack,
) -> clustine_world::ItemStack {
    let components = world_components(&stack.components).unwrap_or_else(|refusal| {
        tracing::debug!(%refusal, "a client's stack had components that were refused");
        Components::NONE
    });
    clustine_world::ItemStack {
        item: stack.item,
        count: stack.count,
        components,
    }
}

#[cfg(test)]
mod tests {
    use clustine_protocol::codec::{Reader, Writer};

    use super::*;

    fn item(name: &str) -> i32 {
        item_id_by_name(name).expect("the item exists")
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

    fn details(amplifier: i32) -> EffectDetails {
        EffectDetails {
            amplifier,
            duration: 200,
            ambient: false,
            visible: true,
            show_icon: true,
            hidden: None,
        }
    }

    fn of(components: &[Component]) -> Components {
        let mut patch = Components::NONE;
        for component in components {
            patch.set(component.clone());
        }
        patch
    }

    fn adding(component: WireComponent) -> WireComponents {
        WireComponents {
            added: vec![component],
            removed: Vec::new(),
        }
    }

    fn refusal(component: WireComponent) -> Refusal {
        world_components(&adding(component)).expect_err("the component is refused")
    }

    /// One component of every kind, each with fields away from their defaults.
    fn every_kind() -> Vec<Component> {
        vec![
            Component::PotionContents(PotionContents {
                potion: Some("minecraft:swiftness".into()),
                custom_colour: Some(0x00FF_0000),
                custom_effects: vec![
                    effect("minecraft:speed", 1, 200),
                    EffectInstance {
                        ambient: true,
                        visible: false,
                        show_icon: false,
                        ..effect("minecraft:poison", 255, -1)
                    },
                ],
                custom_name: Some("fancy".into()),
            }),
            Component::PotionDurationScale(Float(0.25)),
            Component::ChargedProjectiles(vec![
                ItemStack::new(item("minecraft:arrow"), 1),
                ItemStack::new(item("minecraft:firework_rocket"), 3),
            ]),
            Component::Enchantments(vec![
                ("minecraft:sharpness".into(), 3),
                ("minecraft:unbreaking".into(), 255),
            ]),
            Component::UseCooldown(UseCooldown {
                seconds: Float(2.5),
                group: Some("minecraft:ender_pearl".into()),
            }),
            Component::Fireworks(Fireworks {
                flight_duration: 3,
                explosions: vec![FireworkExplosion {
                    shape: FireworkShape::Creeper,
                    colours: vec![0x00FF_0000, 0xFF],
                    fade_colours: vec![0xFF00],
                    trail: true,
                    twinkle: true,
                }],
            }),
            Component::Damage(5),
            Component::IntangibleProjectile,
            Component::BannerPatterns(vec![
                BannerLayer {
                    pattern: "minecraft:stripe_bottom".into(),
                    colour: "black".into(),
                },
                BannerLayer {
                    pattern: "minecraft:base".into(),
                    colour: "white".into(),
                },
            ]),
            Component::ItemName(
                r#"{"text":"a","extra":["b",{"translate":"c","bold":true}],"color":"gold","italic":false}"#
                    .into(),
            ),
            Component::Rarity(Rarity::Epic),
            Component::TooltipDisplay(TooltipDisplay {
                hide_tooltip: true,
                hidden_components: vec!["minecraft:enchantments".into(), "minecraft:waxed".into()],
            }),
            Component::SuspiciousStewEffects(vec![
                ("minecraft:night_vision".into(), 100),
                ("minecraft:saturation".into(), 7),
            ]),
            Component::OminousBottleAmplifier(4),
            Component::ChickenVariant("minecraft:warm".into()),
            Component::StoredEnchantments(vec![
                ("minecraft:efficiency".into(), 5),
                ("minecraft:mending".into(), 1),
            ]),
            Component::DyedColor(-6_265_536),
        ]
    }

    /// The wire's patch through its bytes and back, as a server writes and reads it.
    fn through_the_bytes(components: &WireComponents) -> WireComponents {
        let mut writer = Writer::new();
        writer.put_components(components);
        let bytes = writer.into_bytes();
        let mut reader = Reader::new(&bytes);
        let read = reader
            .components_clientbound()
            .expect("the patch reads back");
        assert_eq!(reader.remaining(), 0);
        read
    }

    fn round_trip(components: &Components) -> Components {
        let wire = through_the_bytes(&wire_components(components));
        world_components(&wire).expect("the world's own components are taken back")
    }

    #[test]
    fn every_kind_round_trips_from_the_world_through_the_wire() {
        let all = every_kind();
        assert_eq!(all.len(), Kind::ALL.len());
        for component in all {
            let components = of(std::slice::from_ref(&component));
            assert_eq!(round_trip(&components), components, "{component:?}");
        }
    }

    #[test]
    fn a_plain_name_and_a_translation_round_trip_as_the_game_writes_them() {
        for (json, nbt) in [
            ("\"Hi\"", Nbt::String("Hi".into())),
            (
                r#"{"translate":"block.minecraft.ominous_banner"}"#,
                Nbt::Compound(vec![(
                    "translate".into(),
                    Nbt::String("block.minecraft.ominous_banner".into()),
                )]),
            ),
        ] {
            let components = of(&[Component::ItemName(json.into())]);
            let wire = wire_components(&components);
            assert_eq!(
                wire.added,
                vec![WireComponent::ItemName(Text::from_nbt(nbt))]
            );
            assert_eq!(round_trip(&components), components);
        }
    }

    #[test]
    fn a_name_with_siblings_of_mixed_tags_wraps_those_that_are_not_compounds() {
        let components = of(&[Component::ItemName(
            r#"{"text":"a","extra":["b",{"text":"c","bold":true}]}"#.into(),
        )]);
        let [WireComponent::ItemName(text)] = &wire_components(&components).added[..] else {
            panic!("one item name");
        };
        let Some(Nbt::List(extra)) = text.to_nbt().get("extra").cloned() else {
            panic!("a list of siblings");
        };
        assert_eq!(
            extra,
            vec![
                Nbt::Compound(vec![(String::new(), Nbt::String("b".into()))]),
                Nbt::Compound(vec![
                    ("text".into(), Nbt::String("c".into())),
                    ("bold".into(), Nbt::Byte(1)),
                ]),
            ]
        );
    }

    #[test]
    fn a_stack_of_every_kind_round_trips_inside_a_crossbow_s_charge() {
        let arrow = ItemStack {
            item: item("minecraft:arrow"),
            count: 1,
            components: of(&every_kind()),
        };
        let mut removing = ItemStack::new(item("minecraft:tipped_arrow"), 2);
        removing.components.remove(Kind::PotionContents);
        removing.components.remove(Kind::PotionDurationScale);
        let crossbow = of(&[Component::ChargedProjectiles(vec![arrow, removing])]);
        assert_eq!(round_trip(&crossbow), crossbow);
    }

    #[test]
    fn removed_kinds_go_by_their_ids_and_come_back() {
        let mut components = Components::NONE;
        for kind in Kind::ALL {
            components.remove(kind);
        }
        let wire = wire_components(&components);
        assert_eq!(wire.removed, WireComponent::IDS.to_vec());
        for (kind, id) in Kind::ALL.into_iter().zip(&wire.removed) {
            assert_eq!(data_component_type::NAMES[*id as usize], kind.name());
        }
        assert_eq!(round_trip(&components), components);
    }

    #[test]
    fn a_swiftness_potion_goes_by_its_number_in_the_potion_registry() {
        let components = of(&[Component::PotionContents(PotionContents {
            potion: Some("minecraft:swiftness".into()),
            ..PotionContents::default()
        })]);
        let [WireComponent::PotionContents(contents)] = &wire_components(&components).added[..]
        else {
            panic!("one potion's contents");
        };
        assert_eq!(contents.potion, Some(13));
        assert_eq!(potion::NAMES[13], "minecraft:swiftness");
    }

    #[test]
    fn sharpness_goes_by_its_place_in_the_enchantments_the_edge_sends() {
        let components = of(&[Component::Enchantments(vec![(
            "minecraft:sharpness".into(),
            5,
        )])]);
        let added = wire_components(&components).added;
        assert_eq!(added, vec![WireComponent::Enchantments(vec![(33, 5)])]);
        // The edge sends the registry's entries in this order, and a client numbers them
        // so.
        assert_eq!(enchantments().entries[33], "minecraft:sharpness");
    }

    #[test]
    fn a_book_s_stored_mending_goes_by_its_place_as_an_enchantment_does() {
        let components = of(&[Component::StoredEnchantments(vec![(
            "minecraft:mending".into(),
            1,
        )])]);
        let added = wire_components(&components).added;
        let mending = enchantments()
            .id_of("minecraft:mending")
            .expect("mending is an enchantment");
        assert_eq!(
            added,
            vec![WireComponent::StoredEnchantments(vec![(mending, 1)])]
        );
        assert_eq!(
            world_components(&adding(added[0].clone())).map(|world| world == components),
            Ok(true)
        );
        refusal(WireComponent::StoredEnchantments(vec![(mending, 0)]));
        refusal(WireComponent::StoredEnchantments(vec![(
            enchantments().entries.len() as i32,
            1,
        )]));
    }

    #[test]
    fn a_dyed_colour_crosses_as_its_int() {
        let components = of(&[Component::DyedColor(-6_265_536)]);
        assert_eq!(
            wire_components(&components).added,
            vec![WireComponent::DyedColor(-6_265_536)]
        );
        assert_eq!(
            world_components(&adding(WireComponent::DyedColor(-6_265_536))),
            Ok(components)
        );
    }

    #[test]
    fn the_warm_chicken_goes_by_its_place_in_the_variants_the_edge_sends() {
        let components = of(&[Component::ChickenVariant("minecraft:warm".into())]);
        assert_eq!(
            wire_components(&components).added,
            vec![WireComponent::ChickenVariant(2)]
        );
        assert_eq!(chicken_variants().entries[2], "minecraft:warm");
    }

    #[test]
    fn a_potion_the_registry_does_not_have_is_refused() {
        refusal(WireComponent::PotionContents(wire::PotionContents {
            potion: Some(potion::NAMES.len() as i32),
            ..wire::PotionContents::default()
        }));
    }

    #[test]
    fn an_effect_the_registry_does_not_have_is_refused() {
        refusal(WireComponent::PotionContents(wire::PotionContents {
            custom_effects: vec![wire::EffectInstance {
                effect: mob_effect::NAMES.len() as i32,
                details: details(0),
            }],
            ..wire::PotionContents::default()
        }));
        refusal(WireComponent::SuspiciousStewEffects(vec![(-1, 100)]));
    }

    #[test]
    fn an_enchantment_the_registry_does_not_have_is_refused() {
        refusal(WireComponent::Enchantments(vec![(
            enchantments().entries.len() as i32,
            1,
        )]));
    }

    #[test]
    fn a_banner_pattern_the_registry_does_not_have_is_refused() {
        refusal(WireComponent::BannerPatterns(vec![wire::BannerLayer {
            pattern: BannerPattern::Registered(banner_patterns().entries.len() as i32),
            colour: 0,
        }]));
    }

    #[test]
    fn a_chicken_variant_the_registry_does_not_have_is_refused() {
        refusal(WireComponent::ChickenVariant(
            chicken_variants().entries.len() as i32,
        ));
    }

    #[test]
    fn an_enchantment_level_outside_1_to_255_is_refused() {
        for level in [0, -1, 256] {
            refusal(WireComponent::Enchantments(vec![(33, level)]));
        }
        assert!(world_components(&adding(WireComponent::Enchantments(vec![(33, 255)]))).is_ok());
    }

    #[test]
    fn a_dye_colour_outside_0_to_15_is_refused() {
        for colour in [-1, 16] {
            refusal(WireComponent::BannerPatterns(vec![wire::BannerLayer {
                pattern: BannerPattern::Registered(0),
                colour,
            }]));
        }
    }

    #[test]
    fn a_banner_pattern_given_whole_is_refused() {
        let refused = refusal(WireComponent::BannerPatterns(vec![wire::BannerLayer {
            pattern: BannerPattern::Direct {
                asset_id: "minecraft:base".into(),
                translation_key: "block.minecraft.banner.base".into(),
            },
            colour: 0,
        }]));
        assert!(refused.0.contains("by name"), "{refused}");
    }

    #[test]
    fn an_effect_with_a_hidden_one_is_refused() {
        refusal(WireComponent::PotionContents(wire::PotionContents {
            custom_effects: vec![wire::EffectInstance {
                effect: 0,
                details: EffectDetails {
                    hidden: Some(Box::new(details(0))),
                    ..details(1)
                },
            }],
            ..wire::PotionContents::default()
        }));
    }

    #[test]
    fn an_effect_amplifier_outside_0_to_255_is_refused() {
        for amplifier in [-1, 256] {
            refusal(WireComponent::PotionContents(wire::PotionContents {
                custom_effects: vec![wire::EffectInstance {
                    effect: 0,
                    details: details(amplifier),
                }],
                ..wire::PotionContents::default()
            }));
        }
    }

    #[test]
    fn a_rocket_s_flight_outside_0_to_255_is_refused() {
        for flight_duration in [-1, 256] {
            refusal(WireComponent::Fireworks(wire::Fireworks {
                flight_duration,
                explosions: Vec::new(),
            }));
        }
    }

    #[test]
    fn negative_damage_is_refused() {
        refusal(WireComponent::Damage(-1));
        assert!(world_components(&adding(WireComponent::Damage(0))).is_ok());
    }

    #[test]
    fn an_ominous_bottle_s_amplifier_outside_0_to_4_is_refused() {
        for amplifier in [-1, 5] {
            refusal(WireComponent::OminousBottleAmplifier(amplifier));
        }
    }

    #[test]
    fn a_cooldown_of_no_time_is_refused() {
        for seconds in [0.0, -1.0, f32::NAN] {
            refusal(WireComponent::UseCooldown(wire::UseCooldown {
                seconds,
                group: None,
            }));
        }
    }

    #[test]
    fn a_cooldown_group_that_is_not_an_identifier_is_refused() {
        refusal(WireComponent::UseCooldown(wire::UseCooldown {
            seconds: 1.0,
            group: Some("Not An Identifier".into()),
        }));
    }

    #[test]
    fn a_cooldown_group_without_a_namespace_is_kept_in_the_game_s() {
        let world = world_components(&adding(WireComponent::UseCooldown(wire::UseCooldown {
            seconds: 1.0,
            group: Some("ender_pearl".into()),
        })))
        .expect("taken");
        assert_eq!(
            world.get(Kind::UseCooldown),
            Some(&Component::UseCooldown(UseCooldown {
                seconds: Float(1.0),
                group: Some("minecraft:ender_pearl".into()),
            }))
        );
    }

    #[test]
    fn a_negative_duration_scale_is_refused() {
        refusal(WireComponent::PotionDurationScale(-0.5));
        refusal(WireComponent::PotionDurationScale(f32::NAN));
        assert!(world_components(&adding(WireComponent::PotionDurationScale(0.0))).is_ok());
    }

    fn charge(stack: WireStack) -> WireComponent {
        WireComponent::ChargedProjectiles(vec![stack])
    }

    fn arrows(count: i32) -> WireStack {
        WireStack {
            item: item("minecraft:arrow"),
            count,
            components: WireComponents::default(),
        }
    }

    #[test]
    fn a_charged_stack_of_an_item_that_is_none_is_refused() {
        refusal(charge(WireStack {
            item: ITEMS.len() as i32,
            ..arrows(1)
        }));
        refusal(charge(WireStack {
            item: -1,
            ..arrows(1)
        }));
    }

    #[test]
    fn a_charged_stack_of_air_is_refused() {
        refusal(charge(WireStack {
            item: item("minecraft:air"),
            ..arrows(1)
        }));
    }

    #[test]
    fn a_charged_stack_counted_outside_1_to_99_is_refused() {
        for count in [0, -1, 100] {
            refusal(charge(arrows(count)));
        }
    }

    #[test]
    fn a_charged_stack_larger_than_its_item_stacks_is_refused() {
        refusal(charge(WireStack {
            item: item("minecraft:ender_pearl"),
            ..arrows(17)
        }));
        assert!(world_components(&adding(charge(arrows(64)))).is_ok());
    }

    #[test]
    fn a_stack_inside_a_charged_stack_is_checked_too() {
        let inner = arrows(0);
        let outer = WireStack {
            components: adding(charge(inner)),
            ..arrows(1)
        };
        refusal(charge(outer));
        let outer = WireStack {
            components: adding(WireComponent::Damage(-1)),
            ..arrows(1)
        };
        refusal(charge(outer));
    }

    #[test]
    fn a_hidden_component_kind_outside_the_registry_is_refused() {
        refusal(WireComponent::TooltipDisplay(wire::TooltipDisplay {
            hide_tooltip: false,
            hidden_components: vec![data_component_type::NAMES.len() as i32],
        }));
    }

    #[test]
    fn a_hidden_component_kind_outside_clustine_s_set_is_kept_by_name() {
        let waxed = position(&data_component_type::NAMES, "minecraft:waxed").expect("a kind");
        assert!(!WireComponent::IDS.contains(&waxed));
        let world = world_components(&adding(WireComponent::TooltipDisplay(
            wire::TooltipDisplay {
                hide_tooltip: false,
                hidden_components: vec![waxed],
            },
        )))
        .expect("taken");
        assert_eq!(
            world.get(Kind::TooltipDisplay),
            Some(&Component::TooltipDisplay(TooltipDisplay {
                hide_tooltip: false,
                hidden_components: vec!["minecraft:waxed".into()],
            }))
        );
    }

    #[test]
    fn an_item_name_the_world_s_reader_does_not_read_is_refused() {
        // A number is no text; a key the world does not keep (a click event) is refused
        // rather than kept in part.
        refusal(WireComponent::ItemName(Text::from_nbt(Nbt::Int(3))));
        refusal(WireComponent::ItemName(Text::from_nbt(Nbt::Compound(
            vec![
                ("text".into(), Nbt::String("a".into())),
                ("click_event".into(), Nbt::Compound(Vec::new())),
            ],
        ))));
    }

    #[test]
    fn an_item_name_read_from_nbt_is_kept_as_the_game_s_json() {
        let world = world_components(&adding(WireComponent::ItemName(Text::from_nbt(
            Nbt::Compound(vec![
                ("text".into(), Nbt::String("a".into())),
                ("bold".into(), Nbt::Byte(1)),
                (
                    "extra".into(),
                    Nbt::List(vec![
                        Nbt::Compound(vec![(String::new(), Nbt::String("b".into()))]),
                        Nbt::Compound(vec![("translate".into(), Nbt::String("c".into()))]),
                    ]),
                ),
            ]),
        ))))
        .expect("taken");
        assert_eq!(
            world.get(Kind::ItemName),
            Some(&Component::ItemName(
                r#"{"text":"a","extra":["b",{"translate":"c"}],"bold":true}"#.into()
            ))
        );
    }

    #[test]
    fn a_removed_kind_outside_the_set_is_let_go() {
        let waxed = position(&data_component_type::NAMES, "minecraft:waxed").expect("a kind");
        let world = world_components(&WireComponents {
            added: vec![WireComponent::Damage(2)],
            removed: vec![waxed, kind_id(Kind::Rarity)],
        })
        .expect("taken");
        let mut expected = of(&[Component::Damage(2)]);
        expected.remove(Kind::Rarity);
        assert_eq!(world, expected);
    }

    #[test]
    fn a_kind_added_twice_is_the_one_read_last() {
        let world = world_components(&WireComponents {
            added: vec![
                WireComponent::Damage(1),
                WireComponent::Rarity(wire::Rarity::Rare),
                WireComponent::Damage(7),
            ],
            removed: Vec::new(),
        })
        .expect("taken");
        assert_eq!(
            world,
            of(&[Component::Damage(7), Component::Rarity(Rarity::Rare)])
        );
    }

    #[test]
    fn a_kind_added_and_removed_is_removed() {
        let world = world_components(&WireComponents {
            added: vec![WireComponent::Damage(1)],
            removed: vec![kind_id(Kind::Damage)],
        })
        .expect("taken");
        assert!(world.removes(Kind::Damage));
        assert_eq!(world.get(Kind::Damage), None);
    }

    #[test]
    fn an_enchantment_given_twice_has_the_level_given_last() {
        let world = world_components(&adding(WireComponent::Enchantments(vec![
            (33, 1),
            (40, 2),
            (33, 4),
        ])))
        .expect("taken");
        assert_eq!(
            world.get(Kind::Enchantments),
            Some(&Component::Enchantments(vec![
                ("minecraft:sharpness".into(), 4),
                ("minecraft:unbreaking".into(), 2),
            ]))
        );
    }

    #[test]
    fn a_patch_of_nothing_is_none() {
        assert_eq!(
            wire_components(&Components::NONE),
            WireComponents::default()
        );
        assert_eq!(
            world_components(&WireComponents::default()),
            Ok(Components::NONE)
        );
    }
}
