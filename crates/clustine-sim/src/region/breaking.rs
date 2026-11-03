//! How fast a survival player breaks a block (`docs/groundwork/v-plan.md`, sections 2.6
//! and 4.5): the game's `BlockBehaviour.getDestroyProgress` and `Player.getDestroySpeed`
//! and `hasCorrectToolForDrops`, as functions of what they read, so that the region's
//! digging asks them each tick as the game's `incrementDestroyProgress` does.
//!
//! The held item's tool is its default `minecraft:tool`: a stack here carries no tool
//! component of its own, which only commands give in the game.

use clustine_data::{BlockState, ItemComponents};
use clustine_world::ItemStack;

/// What `Player.getDestroySpeed` reads of the player beside the block: each as the game
/// holds it at the time of asking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Digger<'a> {
    /// The stack in the selected hotbar slot, if any.
    pub held: Option<&'a ItemStack>,
    /// `Attributes.MINING_EFFICIENCY`, which Efficiency raises while the tool is held.
    pub mining_efficiency: f64,
    /// `MobEffectUtil.getDigSpeedAmplification` where `hasDigSpeed`: the greater
    /// amplifier of Haste and Conduit Power, if either is on the player.
    pub dig_speed_amplifier: Option<i32>,
    /// The amplifier of Mining Fatigue, if it is on the player.
    pub mining_fatigue: Option<i32>,
    /// `Attributes.BLOCK_BREAK_SPEED`.
    pub block_break_speed: f64,
    /// `Attributes.SUBMERGED_MINING_SPEED` where the player's eyes are in water.
    pub submerged_mining_speed: Option<f64>,
    pub on_ground: bool,
}

/// The default tool component of the stack's item (`ItemStack.get(TOOL)`).
fn tool_of(held: Option<&ItemStack>) -> Option<&'static clustine_data::Tool> {
    held.filter(|stack| stack.count > 0)
        .and_then(|stack| ItemComponents::of(stack.item).tool.as_ref())
}

/// `Player.hasCorrectToolForDrops`: the block does not need the right tool, or the held
/// item's tool says it is the right one (`ItemStack.isCorrectToolForDrops`).
pub(crate) fn has_correct_tool_for_drops(held: Option<&ItemStack>, state: BlockState) -> bool {
    let needs = state
        .breaking()
        .is_some_and(|breaking| breaking.requires_correct_tool);
    !needs || tool_of(held).is_some_and(|tool| tool.is_correct_for_drops(state))
}

impl Digger<'_> {
    /// `Player.getDestroySpeed @0-140`, its float arithmetic as the game's.
    pub(crate) fn destroy_speed(&self, state: BlockState) -> f32 {
        // `ItemStack.getDestroySpeed`: the tool's speed, 1 without one.
        let mut speed = tool_of(self.held).map_or(1.0, |tool| tool.mining_speed(state));
        if speed > 1.0 {
            speed += self.mining_efficiency as f32;
        }
        if let Some(amplifier) = self.dig_speed_amplifier {
            speed *= 1.0 + (amplifier + 1) as f32 * 0.2;
        }
        if let Some(amplifier) = self.mining_fatigue {
            speed *= 0.3_f64.powf(f64::from(amplifier + 1)) as f32;
        }
        speed *= self.block_break_speed as f32;
        if let Some(factor) = self.submerged_mining_speed {
            speed *= factor as f32;
        }
        if !self.on_ground {
            speed /= 5.0;
        }
        speed
    }

    /// `BlockBehaviour.getDestroyProgress @0-49`: what a tick of digging does of the
    /// block, 1 being all of it; 0 for a block that does not break (hardness -1).
    pub(crate) fn destroy_progress(&self, state: BlockState) -> f32 {
        let Some(hardness) = state.breaking().map(|breaking| breaking.hardness) else {
            return 0.0;
        };
        if hardness == -1.0 {
            return 0.0;
        }
        let divisor = if has_correct_tool_for_drops(self.held, state) {
            30
        } else {
            100
        };
        self.destroy_speed(state) / hardness / divisor as f32
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{BlockState, blocks, items};
    use clustine_world::ItemStack;

    use super::{Digger, has_correct_tool_for_drops};

    fn bare<'a>(held: Option<&'a ItemStack>) -> Digger<'a> {
        Digger {
            held,
            mining_efficiency: 0.0,
            dig_speed_amplifier: None,
            mining_fatigue: None,
            block_break_speed: 1.0,
            submerged_mining_speed: None,
            on_ground: true,
        }
    }

    fn state(name: &str) -> BlockState {
        BlockState::parse(name).unwrap()
    }

    /// Ticks to break, as the client counts: the first tick at which the progress times
    /// the ticks reaches 1.
    fn ticks(digger: &Digger<'_>, state: BlockState) -> u32 {
        let progress = digger.destroy_progress(state);
        (1..10_000)
            .find(|ticks| progress * *ticks as f32 >= 1.0)
            .expect("breaks")
    }

    #[test]
    fn a_hand_breaks_dirt_in_fifteen_ticks_and_stone_in_a_hundred_and_fifty() {
        // The wiki's table of breaking times: dirt 0.75 s by hand, stone 7.5 s.
        assert_eq!(ticks(&bare(None), blocks::DIRT), 15);
        assert_eq!(ticks(&bare(None), blocks::STONE), 150);
    }

    #[test]
    fn a_wooden_pickaxe_breaks_stone_in_twenty_three_ticks_and_makes_it_drop() {
        let pickaxe = ItemStack::new(items::WOODEN_PICKAXE, 1);
        assert_eq!(ticks(&bare(Some(&pickaxe)), blocks::STONE), 23);
        assert!(has_correct_tool_for_drops(Some(&pickaxe), blocks::STONE));
        assert!(!has_correct_tool_for_drops(None, blocks::STONE));
        assert!(has_correct_tool_for_drops(None, blocks::DIRT));
    }

    #[test]
    fn a_diamond_pickaxe_breaks_obsidian_in_a_hundred_and_eighty_eight_ticks() {
        let pickaxe = ItemStack::new(items::DIAMOND_PICKAXE, 1);
        // 8 / 50 / 30 a tick: 9.4 s.
        assert_eq!(
            ticks(&bare(Some(&pickaxe)), state("minecraft:obsidian")),
            188
        );
    }

    #[test]
    fn bedrock_never_breaks_and_a_flower_at_once() {
        assert_eq!(bare(None).destroy_progress(blocks::BEDROCK), 0.0);
        assert!(
            bare(None)
                .destroy_progress(state("minecraft:poppy"))
                .is_infinite()
        );
    }

    #[test]
    fn off_the_ground_a_fifth_and_haste_two_two_fifths_more() {
        let flying = Digger {
            on_ground: false,
            ..bare(None)
        };
        assert_eq!(flying.destroy_speed(blocks::DIRT), 0.2);
        let hasted = Digger {
            dig_speed_amplifier: Some(1),
            ..bare(None)
        };
        assert_eq!(hasted.destroy_speed(blocks::DIRT), 1.4);
        let tired = Digger {
            mining_fatigue: Some(0),
            ..bare(None)
        };
        assert_eq!(tired.destroy_speed(blocks::DIRT), 0.3);
    }

    #[test]
    fn efficiency_adds_only_where_the_tool_is_faster_than_a_hand() {
        let pickaxe = ItemStack::new(items::IRON_PICKAXE, 1);
        let efficient = Digger {
            mining_efficiency: 26.0,
            ..bare(Some(&pickaxe))
        };
        assert_eq!(efficient.destroy_speed(blocks::STONE), 32.0);
        assert_eq!(efficient.destroy_speed(blocks::DIRT), 1.0);
    }

    #[test]
    fn a_sword_breaks_a_cobweb_fast_and_makes_it_drop() {
        let sword = ItemStack::new(items::IRON_SWORD, 1);
        let cobweb = state("minecraft:cobweb");
        assert_eq!(bare(Some(&sword)).destroy_speed(cobweb), 15.0);
        assert!(has_correct_tool_for_drops(Some(&sword), cobweb));
    }
}
