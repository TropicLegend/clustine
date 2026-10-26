//! Each component of the set as the wire carries it, and its stream codec both ways.
//!
//! The codecs are the game's: `PotionContents`, `MobEffectInstance` and its `Details`,
//! `ChargedProjectiles` (with `ItemStackTemplate`), `ItemEnchantments`, `UseCooldown`,
//! `Fireworks`, `FireworkExplosion`, `BannerPatternLayers`, `Rarity`, `TooltipDisplay`,
//! `SuspiciousStewEffects`, `OminousBottleAmplifier`, the text component of
//! `item_name`, and `chicken/variant`'s holder. Lists the game bounds are bounded here
//! by the same maxima, and the writers write what they are given: the edge keeps a
//! value within the game's limits before it is written.

use super::{MAX_NESTING, OUTSIDE_THE_SET, WireComponents, data_component_type, read_patch};
use crate::codec::{DecodeError, MAX_STRING_LENGTH, Reader, Writer};
use crate::text::Text;

/// The most stacks a crossbow's `charged_projectiles` holds (n9 `ChargedProjectiles`,
/// `ByteBufCodecs.list(1024)`).
pub const MAX_CHARGED_PROJECTILES: usize = 1024;

/// The most explosions a rocket's `fireworks` holds (`Fireworks.MAX_EXPLOSIONS`).
pub const MAX_EXPLOSIONS: usize = 256;

/// The highest level the game's `ItemEnchantments` accepts.
pub const MAX_ENCHANTMENT_LEVEL: i32 = 255;

/// One component's value.
#[derive(Debug, Clone, PartialEq)]
pub enum WireComponent {
    PotionContents(PotionContents),
    PotionDurationScale(f32),
    ChargedProjectiles(Vec<WireStack>),
    /// Enchantments by their number in the `enchantment` registry the edge sends, each
    /// with its level, in the order read.
    Enchantments(Vec<(i32, i32)>),
    UseCooldown(UseCooldown),
    Fireworks(Fireworks),
    Damage(i32),
    /// A marker, with no bytes on the wire.
    IntangibleProjectile,
    BannerPatterns(Vec<BannerLayer>),
    ItemName(Text),
    Rarity(Rarity),
    TooltipDisplay(TooltipDisplay),
    /// Effects by their number in the `mob_effect` registry, each with its duration in
    /// ticks.
    SuspiciousStewEffects(Vec<(i32, i32)>),
    OminousBottleAmplifier(i32),
    ChickenVariant(ChickenVariant),
}

impl WireComponent {
    /// The ids of the set's kinds, in the order of the world's `components::Kind`.
    pub const IDS: [i32; 15] = [
        data_component_type::POTION_CONTENTS,
        data_component_type::POTION_DURATION_SCALE,
        data_component_type::CHARGED_PROJECTILES,
        data_component_type::ENCHANTMENTS,
        data_component_type::USE_COOLDOWN,
        data_component_type::FIREWORKS,
        data_component_type::DAMAGE,
        data_component_type::INTANGIBLE_PROJECTILE,
        data_component_type::BANNER_PATTERNS,
        data_component_type::ITEM_NAME,
        data_component_type::RARITY,
        data_component_type::TOOLTIP_DISPLAY,
        data_component_type::SUSPICIOUS_STEW_EFFECTS,
        data_component_type::OMINOUS_BOTTLE_AMPLIFIER,
        data_component_type::CHICKEN_VARIANT,
    ];

    /// The id of its kind in the `data_component_type` registry.
    pub fn id(&self) -> i32 {
        match self {
            Self::PotionContents(_) => data_component_type::POTION_CONTENTS,
            Self::PotionDurationScale(_) => data_component_type::POTION_DURATION_SCALE,
            Self::ChargedProjectiles(_) => data_component_type::CHARGED_PROJECTILES,
            Self::Enchantments(_) => data_component_type::ENCHANTMENTS,
            Self::UseCooldown(_) => data_component_type::USE_COOLDOWN,
            Self::Fireworks(_) => data_component_type::FIREWORKS,
            Self::Damage(_) => data_component_type::DAMAGE,
            Self::IntangibleProjectile => data_component_type::INTANGIBLE_PROJECTILE,
            Self::BannerPatterns(_) => data_component_type::BANNER_PATTERNS,
            Self::ItemName(_) => data_component_type::ITEM_NAME,
            Self::Rarity(_) => data_component_type::RARITY,
            Self::TooltipDisplay(_) => data_component_type::TOOLTIP_DISPLAY,
            Self::SuspiciousStewEffects(_) => data_component_type::SUSPICIOUS_STEW_EFFECTS,
            Self::OminousBottleAmplifier(_) => data_component_type::OMINOUS_BOTTLE_AMPLIFIER,
            Self::ChickenVariant(_) => data_component_type::CHICKEN_VARIANT,
        }
    }
}

/// `PotionContents`: a potion by its number in the `potion` registry, a colour of its
/// own, effects of its own and a name of its own, each optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PotionContents {
    pub potion: Option<i32>,
    /// An RGB colour that overrides the effects' mix.
    pub custom_colour: Option<i32>,
    pub custom_effects: Vec<EffectInstance>,
    pub custom_name: Option<String>,
}

/// `MobEffectInstance`: an effect by its number in the `mob_effect` registry and its
/// settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectInstance {
    pub effect: i32,
    pub details: EffectDetails,
}

/// `MobEffectInstance.Details`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDetails {
    pub amplifier: i32,
    /// In ticks; -1 for one without end.
    pub duration: i32,
    pub ambient: bool,
    /// The game's `showParticles`.
    pub visible: bool,
    pub show_icon: bool,
    /// The weaker instance kept under this one, which the world leaves out.
    pub hidden: Option<Box<EffectDetails>>,
}

/// A stack inside a component (`ItemStackTemplate`): never empty.
#[derive(Debug, Clone, PartialEq)]
pub struct WireStack {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
    pub components: WireComponents,
}

/// `UseCooldown`.
#[derive(Debug, Clone, PartialEq)]
pub struct UseCooldown {
    pub seconds: f32,
    /// An identifier, as read.
    pub group: Option<String>,
}

/// `Fireworks`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fireworks {
    pub flight_duration: i32,
    pub explosions: Vec<FireworkExplosion>,
}

/// `FireworkExplosion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireworkExplosion {
    pub shape: FireworkShape,
    /// RGB colours.
    pub colours: Vec<i32>,
    pub fade_colours: Vec<i32>,
    pub trail: bool,
    pub twinkle: bool,
}

/// `FireworkExplosion.Shape`, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireworkShape {
    SmallBall,
    LargeBall,
    Star,
    Creeper,
    Burst,
}

impl FireworkShape {
    const BY_ID: [FireworkShape; 5] = [
        Self::SmallBall,
        Self::LargeBall,
        Self::Star,
        Self::Creeper,
        Self::Burst,
    ];

    pub fn id(self) -> i32 {
        self as i32
    }

    /// The shape of `id`; one out of range is the first, as the game's
    /// `ByIdMap.OutOfBoundsStrategy.ZERO` has it.
    pub fn of_id(id: i32) -> Self {
        by_id_or_first(&Self::BY_ID, id)
    }
}

/// One layer of `BannerPatternLayers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BannerLayer {
    pub pattern: BannerPattern,
    /// A `DyeColor` by id, 0 (white) to 15 (black).
    pub colour: i32,
}

/// A banner pattern's holder: one of the `banner_pattern` registry the edge sends, or
/// one given whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerPattern {
    Registered(i32),
    Direct {
        asset_id: String,
        translation_key: String,
    },
}

/// `Rarity`, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
}

impl Rarity {
    const BY_ID: [Rarity; 4] = [Self::Common, Self::Uncommon, Self::Rare, Self::Epic];

    pub fn id(self) -> i32 {
        self as i32
    }

    /// The rarity of `id`; one out of range is common (`OutOfBoundsStrategy.ZERO`).
    pub fn of_id(id: i32) -> Self {
        by_id_or_first(&Self::BY_ID, id)
    }
}

/// The number of `DyeColor`s, whose ids out of range read as white.
const DYE_COLOURS: i32 = 16;

/// `TooltipDisplay`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TooltipDisplay {
    pub hide_tooltip: bool,
    /// Ids in the `data_component_type` registry, of any kind, in the order read.
    pub hidden_components: Vec<i32>,
}

/// `chicken/variant`'s `EitherHolder`: a variant of the `chicken_variant` registry the
/// edge sends, or a variant by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChickenVariant {
    Registered(i32),
    Named(String),
}

fn by_id_or_first<T: Copy>(all: &[T], id: i32) -> T {
    usize::try_from(id)
        .ok()
        .and_then(|id| all.get(id).copied())
        .unwrap_or(all[0])
}

impl Writer {
    /// Writes a component's value by its kind's stream codec, with nothing before it.
    pub fn put_component(&mut self, component: &WireComponent) {
        match component {
            WireComponent::PotionContents(contents) => self.put_potion_contents(contents),
            WireComponent::PotionDurationScale(scale) => self.put_f32(*scale),
            WireComponent::ChargedProjectiles(stacks) => {
                self.put_array(stacks, Writer::put_wire_stack);
            }
            WireComponent::Enchantments(levels) => {
                self.put_array(levels, |w, (enchantment, level)| {
                    w.put_var_int(*enchantment);
                    w.put_var_int(*level);
                });
            }
            WireComponent::UseCooldown(cooldown) => {
                self.put_f32(cooldown.seconds);
                self.put_option(cooldown.group.as_ref(), |w, group| w.put_string(group));
            }
            WireComponent::Fireworks(fireworks) => self.put_fireworks(fireworks),
            WireComponent::Damage(damage) => self.put_var_int(*damage),
            WireComponent::IntangibleProjectile => {}
            WireComponent::BannerPatterns(layers) => {
                self.put_array(layers, Writer::put_banner_layer);
            }
            WireComponent::ItemName(text) => self.put_nbt(&text.to_nbt()),
            WireComponent::Rarity(rarity) => self.put_var_int(rarity.id()),
            WireComponent::TooltipDisplay(display) => {
                self.put_bool(display.hide_tooltip);
                self.put_array(&display.hidden_components, |w, id| w.put_var_int(*id));
            }
            WireComponent::SuspiciousStewEffects(effects) => {
                self.put_array(effects, |w, (effect, duration)| {
                    w.put_var_int(*effect);
                    w.put_var_int(*duration);
                });
            }
            WireComponent::OminousBottleAmplifier(amplifier) => self.put_var_int(*amplifier),
            WireComponent::ChickenVariant(variant) => match variant {
                ChickenVariant::Registered(id) => {
                    self.put_bool(true);
                    self.put_var_int(*id);
                }
                ChickenVariant::Named(name) => {
                    self.put_bool(false);
                    self.put_string(name);
                }
            },
        }
    }

    /// `PotionContents.STREAM_CODEC`: the optional potion, colour (a plain `i32`),
    /// the effects and the name.
    pub fn put_potion_contents(&mut self, contents: &PotionContents) {
        self.put_option(contents.potion.as_ref(), |w, potion| w.put_var_int(*potion));
        self.put_option(contents.custom_colour.as_ref(), |w, colour| {
            w.put_i32(*colour)
        });
        self.put_array(&contents.custom_effects, Writer::put_effect_instance);
        self.put_option(contents.custom_name.as_ref(), |w, name| w.put_string(name));
    }

    /// `MobEffectInstance.STREAM_CODEC`: the effect, then its details.
    pub fn put_effect_instance(&mut self, effect: &EffectInstance) {
        self.put_var_int(effect.effect);
        self.put_effect_details(&effect.details);
    }

    /// `MobEffectInstance.Details.STREAM_CODEC`, the hidden instance after it.
    pub fn put_effect_details(&mut self, details: &EffectDetails) {
        self.put_var_int(details.amplifier);
        self.put_var_int(details.duration);
        self.put_bool(details.ambient);
        self.put_bool(details.visible);
        self.put_bool(details.show_icon);
        self.put_option(details.hidden.as_deref(), Writer::put_effect_details);
    }

    /// `ItemStackTemplate.STREAM_CODEC`: the item, the count, then the patch as a
    /// server writes it.
    pub fn put_wire_stack(&mut self, stack: &WireStack) {
        self.put_var_int(stack.item);
        self.put_var_int(stack.count);
        self.put_components(&stack.components);
    }

    /// `Fireworks.STREAM_CODEC`.
    pub fn put_fireworks(&mut self, fireworks: &Fireworks) {
        self.put_var_int(fireworks.flight_duration);
        self.put_array(&fireworks.explosions, Writer::put_firework_explosion);
    }

    /// `FireworkExplosion.STREAM_CODEC`: the colours as plain `i32`s.
    pub fn put_firework_explosion(&mut self, explosion: &FireworkExplosion) {
        self.put_var_int(explosion.shape.id());
        self.put_array(&explosion.colours, |w, colour| w.put_i32(*colour));
        self.put_array(&explosion.fade_colours, |w, colour| w.put_i32(*colour));
        self.put_bool(explosion.trail);
        self.put_bool(explosion.twinkle);
    }

    /// `BannerPatternLayers.Layer.STREAM_CODEC`: the pattern's holder, then the colour.
    pub fn put_banner_layer(&mut self, layer: &BannerLayer) {
        match &layer.pattern {
            // A holder of the registry is its id plus one; nought says one given whole.
            // The edge writes only ids of the registry, far below the largest `i32`.
            BannerPattern::Registered(id) => self.put_var_int(id.wrapping_add(1)),
            BannerPattern::Direct {
                asset_id,
                translation_key,
            } => {
                self.put_var_int(0);
                self.put_string(asset_id);
                self.put_string(translation_key);
            }
        }
        self.put_var_int(layer.colour);
    }
}

impl Reader<'_> {
    /// Reads the value of the component kind `id`, `depth` stacks deep. A kind outside
    /// the set is refused with [`OUTSIDE_THE_SET`].
    pub(crate) fn component(
        &mut self,
        id: i32,
        depth: usize,
    ) -> Result<WireComponent, DecodeError> {
        use data_component_type as kind;
        Ok(match id {
            kind::POTION_CONTENTS => WireComponent::PotionContents(self.potion_contents()?),
            kind::POTION_DURATION_SCALE => WireComponent::PotionDurationScale(self.f32()?),
            kind::CHARGED_PROJECTILES => {
                let count = bounded_count(self, MAX_CHARGED_PROJECTILES, "charged projectiles")?;
                let mut stacks = Vec::with_capacity(count.min(64));
                for _ in 0..count {
                    stacks.push(wire_stack(self, depth)?);
                }
                WireComponent::ChargedProjectiles(stacks)
            }
            kind::ENCHANTMENTS => WireComponent::Enchantments(self.array(|r| {
                let enchantment = r.var_int()?;
                let level = r.var_int()?;
                if !(0..=MAX_ENCHANTMENT_LEVEL).contains(&level) {
                    return Err(DecodeError::InvalidValue {
                        what: "enchantment level",
                        value: level.into(),
                    });
                }
                Ok((enchantment, level))
            })?),
            kind::USE_COOLDOWN => WireComponent::UseCooldown(UseCooldown {
                seconds: self.f32()?,
                group: self.option(checked_identifier)?,
            }),
            kind::FIREWORKS => WireComponent::Fireworks(self.fireworks()?),
            kind::DAMAGE => WireComponent::Damage(self.var_int()?),
            kind::INTANGIBLE_PROJECTILE => WireComponent::IntangibleProjectile,
            kind::BANNER_PATTERNS => {
                WireComponent::BannerPatterns(self.array(Reader::banner_layer)?)
            }
            kind::ITEM_NAME => {
                // The game's tag codec refuses a lone end tag ("Expected non-null
                // compound tag"); whether the rest is a text component is the edge's.
                let value = self.nbt()?.ok_or(DecodeError::InvalidValue {
                    what: "text component",
                    value: 0,
                })?;
                WireComponent::ItemName(Text::from_nbt(value))
            }
            kind::RARITY => WireComponent::Rarity(Rarity::of_id(self.var_int()?)),
            kind::TOOLTIP_DISPLAY => WireComponent::TooltipDisplay(TooltipDisplay {
                hide_tooltip: self.bool()?,
                hidden_components: self.array(super::component_id)?,
            }),
            kind::SUSPICIOUS_STEW_EFFECTS => WireComponent::SuspiciousStewEffects(
                self.array(|r| Ok((r.var_int()?, r.var_int()?)))?,
            ),
            kind::OMINOUS_BOTTLE_AMPLIFIER => {
                WireComponent::OminousBottleAmplifier(self.var_int()?)
            }
            kind::CHICKEN_VARIANT => WireComponent::ChickenVariant(if self.bool()? {
                ChickenVariant::Registered(self.var_int()?)
            } else {
                ChickenVariant::Named(checked_identifier(self)?)
            }),
            _ => {
                return Err(DecodeError::InvalidValue {
                    what: OUTSIDE_THE_SET,
                    value: id.into(),
                });
            }
        })
    }

    /// `PotionContents.STREAM_CODEC`.
    pub fn potion_contents(&mut self) -> Result<PotionContents, DecodeError> {
        Ok(PotionContents {
            potion: self.option(Reader::var_int)?,
            custom_colour: self.option(Reader::i32)?,
            custom_effects: self.array(Reader::effect_instance)?,
            custom_name: self.option(|r| r.string(MAX_STRING_LENGTH))?,
        })
    }

    /// `MobEffectInstance.STREAM_CODEC`.
    pub fn effect_instance(&mut self) -> Result<EffectInstance, DecodeError> {
        Ok(EffectInstance {
            effect: self.var_int()?,
            details: effect_details(self, 0)?,
        })
    }

    /// `Fireworks.STREAM_CODEC`.
    pub fn fireworks(&mut self) -> Result<Fireworks, DecodeError> {
        let flight_duration = self.var_int()?;
        let count = bounded_count(self, MAX_EXPLOSIONS, "firework explosions")?;
        let mut explosions = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            explosions.push(self.firework_explosion()?);
        }
        Ok(Fireworks {
            flight_duration,
            explosions,
        })
    }

    /// `FireworkExplosion.STREAM_CODEC`.
    pub fn firework_explosion(&mut self) -> Result<FireworkExplosion, DecodeError> {
        Ok(FireworkExplosion {
            shape: FireworkShape::of_id(self.var_int()?),
            colours: self.array(Reader::i32)?,
            fade_colours: self.array(Reader::i32)?,
            trail: self.bool()?,
            twinkle: self.bool()?,
        })
    }

    /// `BannerPatternLayers.Layer.STREAM_CODEC`.
    pub fn banner_layer(&mut self) -> Result<BannerLayer, DecodeError> {
        let pattern = match self.var_int()? {
            0 => BannerPattern::Direct {
                asset_id: checked_identifier(self)?,
                translation_key: self.string(MAX_STRING_LENGTH)?,
            },
            id if id > 0 => BannerPattern::Registered(id - 1),
            // No registry has a negative id: the game's `byIdOrThrow` refuses it.
            id => {
                return Err(DecodeError::InvalidValue {
                    what: "banner pattern",
                    value: id.into(),
                });
            }
        };
        let colour = self.var_int()?;
        Ok(BannerLayer {
            pattern,
            colour: if (0..DYE_COLOURS).contains(&colour) {
                colour
            } else {
                0
            },
        })
    }
}

/// `ItemStackTemplate.STREAM_CODEC`, `depth` stacks deep: a stack inside a stack is one
/// deeper.
fn wire_stack(r: &mut Reader<'_>, depth: usize) -> Result<WireStack, DecodeError> {
    if depth >= MAX_NESTING {
        return Err(DecodeError::InvalidValue {
            what: "depth of stacks in stacks",
            value: depth as i64 + 1,
        });
    }
    let item = r.var_int()?;
    let count = r.var_int()?;
    if item == 0 || count <= 0 {
        // A template is never empty: air, or no items, is refused.
        return Err(DecodeError::InvalidValue {
            what: "stack inside a component",
            value: count.into(),
        });
    }
    Ok(WireStack {
        item,
        count,
        components: read_patch(r, depth + 1)?,
    })
}

/// `MobEffectInstance.Details.STREAM_CODEC`, `depth` hidden instances deep.
fn effect_details(r: &mut Reader<'_>, depth: usize) -> Result<EffectDetails, DecodeError> {
    if depth >= MAX_NESTING {
        return Err(DecodeError::InvalidValue {
            what: "depth of hidden effects",
            value: depth as i64 + 1,
        });
    }
    Ok(EffectDetails {
        amplifier: r.var_int()?,
        duration: r.var_int()?,
        ambient: r.bool()?,
        visible: r.bool()?,
        show_icon: r.bool()?,
        hidden: r.option(|r| effect_details(r, depth + 1))?.map(Box::new),
    })
}

/// A list's count, which the game's `ByteBufCodecs.readCount` refuses over `max`.
fn bounded_count(r: &mut Reader<'_>, max: usize, what: &'static str) -> Result<usize, DecodeError> {
    let raw = r.var_int()?;
    if usize::try_from(raw).is_ok_and(|count| count > max) {
        return Err(DecodeError::InvalidValue {
            what,
            value: raw.into(),
        });
    }
    // Negative, or more than the input could hold, is refused here.
    let count = usize::try_from(raw).map_err(|_| DecodeError::NegativeLength(raw))?;
    if count > r.remaining() {
        return Err(DecodeError::LengthExceedsData {
            length: count,
            remaining: r.remaining(),
        });
    }
    Ok(count)
}

/// `Identifier.STREAM_CODEC`: a string the game parses as an identifier, refusing one
/// with a character outside its alphabet. It is kept as written, so that it is written
/// back the same.
fn checked_identifier(r: &mut Reader<'_>) -> Result<String, DecodeError> {
    let text = r.string(MAX_STRING_LENGTH)?;
    let (namespace, path) = text.split_once(':').unwrap_or(("", &text));
    let plain = |c: char| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-' | '.');
    if !namespace.chars().all(plain) || !path.chars().all(|c| plain(c) || c == '/') {
        return Err(DecodeError::InvalidValue {
            what: "identifier",
            value: text.len() as i64,
        });
    }
    Ok(text)
}
