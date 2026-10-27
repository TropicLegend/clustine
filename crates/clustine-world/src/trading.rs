//! A merchant's offers as the world keeps them (`docs/groundwork/n10-plan.md`, section 4;
//! `docs/groundwork/n10-dumps.md`, part B, section 4): the game's `MerchantOffer`,
//! `ItemCost` and `DataComponentExactPredicate`, with the methods of `MerchantOffer` that
//! are only arithmetic on an offer and the stacks a player pays with. Drawing offers is
//! the simulation's (`clustine-sim`'s `trading`); keeping them in a villager, the menu and
//! the packets are others'.
//!
//! The arithmetic is the game's to the type: the price's demand bonus is an `int`
//! product made a `float`, multiplied by the `float` multiplier and floored
//! (`MerchantOffer.getModifiedCostCount`, n10 MerchantOffer 152–192), and `int` sums wrap
//! as Java's do.

use serde::{Deserialize, Serialize};

use crate::{Component, Components, Float, ItemStack};

/// `DataComponentExactPredicate`: components a stack has to have, each equal to the
/// stack's own (`Objects.equals`); those it does not name are not looked at (n10
/// DataComponentExactPredicate 125–152). One of a kind, in the order of
/// [`crate::components::Kind`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactPredicate(pub Vec<Component>);

impl ExactPredicate {
    /// Nothing wanted: every stack passes.
    pub const EMPTY: ExactPredicate = ExactPredicate(Vec::new());

    /// Of `components`, kept one of a kind in the kinds' order (a later one of a kind in
    /// place of an earlier, as the game's map keeps it).
    pub fn of(components: Vec<Component>) -> ExactPredicate {
        let mut kept: Vec<Component> = Vec::with_capacity(components.len());
        for component in components {
            match kept.binary_search_by_key(&component.kind(), Component::kind) {
                Ok(at) => kept[at] = component,
                Err(at) => kept.insert(at, component),
            }
        }
        ExactPredicate(kept)
    }

    /// Whether `stack` has each component named, equal. The stack's value of a kind is
    /// what its patch sets: the one kind the game's trades name (`potion_contents`, the
    /// wandering trader's water bottle) is empty in every item's defaults, so a stack
    /// that has the value has it in its patch.
    pub fn test(&self, stack: &ItemStack) -> bool {
        self.0
            .iter()
            .all(|wanted| stack.components.get(wanted.kind()) == Some(wanted))
    }

    /// The components as a patch (`asPatch`): what the cost's stack shows.
    pub fn as_patch(&self) -> Components {
        let mut patch = Components::NONE;
        for component in &self.0 {
            patch.set(component.clone());
        }
        patch
    }
}

/// `ItemCost`: an item, a count, and the components the paid stack has to have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemCost {
    /// Id in the item registry.
    pub item: i32,
    pub count: i32,
    pub components: ExactPredicate,
}

impl ItemCost {
    /// `itemStack()`: the cost as a stack of its count, its components the patch.
    pub fn stack(&self) -> ItemStack {
        self.stack_of(self.count)
    }

    /// The cost's stack with `count` in place of its own (`copyWithCount`).
    pub fn stack_of(&self, count: i32) -> ItemStack {
        ItemStack {
            item: self.item,
            count,
            components: self.components.as_patch(),
        }
    }

    /// `test(stack)`: the same item, and every component named equal; the count is not
    /// looked at (n10 ItemCost 94–109).
    pub fn test(&self, stack: &ItemStack) -> bool {
        stack.item == self.item && self.components.test(stack)
    }

    /// The most a stack of the cost holds (`getMaxStackSize`): its item's, which no
    /// component a cost names changes.
    fn max_stack_size(&self) -> i32 {
        i32::from(clustine_data::ItemComponents::of(self.item).max_stack_size)
    }
}

/// `MerchantOffer`, every field (n10 MerchantOffer 7–25). Only `uses`, `special_price`
/// and `demand` change after the offer is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    /// `baseCostA`: what is paid in the first slot before the price is modified.
    pub cost_a: ItemCost,
    pub cost_b: Option<ItemCost>,
    pub result: ItemStack,
    pub uses: i32,
    pub max_uses: i32,
    pub reward_exp: bool,
    /// `specialPriceDiff`.
    pub special_price: i32,
    pub demand: i32,
    pub price_multiplier: Float,
    pub xp: i32,
}

impl Offer {
    /// The constructor trades use, `(a, b, result, maxUses, xp, multiplier)`: no uses,
    /// experience rewarded, no special price, no demand.
    pub fn new(
        cost_a: ItemCost,
        cost_b: Option<ItemCost>,
        result: ItemStack,
        max_uses: i32,
        xp: i32,
        price_multiplier: f32,
    ) -> Offer {
        Offer {
            cost_a,
            cost_b,
            result,
            uses: 0,
            max_uses,
            reward_exp: true,
            special_price: 0,
            demand: 0,
            price_multiplier: Float(price_multiplier),
            xp,
        }
    }

    /// `getModifiedCostCount(baseCostA)`: the base count, plus the demand bonus where it
    /// is above nought, plus the special price, kept within 1 and a full stack.
    pub fn modified_cost_count(&self) -> i32 {
        let base = self.cost_a.count;
        // `Mth.floor((float)(count * demand) * priceMultiplier)`: an int product, then
        // floats. A cast from a float saturates as Java's `(int)` of a `floor` does.
        let bonus =
            ((base.wrapping_mul(self.demand) as f32) * self.price_multiplier.0).floor() as i32;
        base.wrapping_add(bonus.max(0))
            .wrapping_add(self.special_price)
            .clamp(1, self.cost_a.max_stack_size().max(1))
    }

    /// `getCostA()`: what the first slot takes, at the modified price.
    pub fn cost_a(&self) -> ItemStack {
        self.cost_a.stack_of(self.modified_cost_count())
    }

    /// `getCostB()`: what the second slot takes, never modified; `None` for the game's
    /// empty stack.
    pub fn cost_b(&self) -> Option<ItemStack> {
        self.cost_b.as_ref().map(ItemCost::stack)
    }

    /// `assemble()`: a copy of what the offer gives.
    pub fn assemble(&self) -> ItemStack {
        self.result.clone()
    }

    /// `satisfiedBy(a, b)` (n10 MerchantOffer 360–401): `a` is the first cost at least at
    /// the modified price, and `b` the second cost at least at its count, or empty where
    /// there is none.
    pub fn satisfied_by(&self, a: Option<&ItemStack>, b: Option<&ItemStack>) -> bool {
        let Some(a) = a.filter(|a| a.count > 0) else {
            return false;
        };
        if !self.cost_a.test(a) || a.count < self.modified_cost_count() {
            return false;
        }
        let b = b.filter(|b| b.count > 0);
        match &self.cost_b {
            Some(cost) => b.is_some_and(|b| cost.test(b) && b.count >= cost.count),
            None => b.is_none(),
        }
    }

    /// `take(a, b)` (n10 MerchantOffer 403–427): where satisfied, the payment is taken
    /// from the two stacks (a stack used up becomes empty) and the answer is true.
    pub fn take(&self, a: &mut Option<ItemStack>, b: &mut Option<ItemStack>) -> bool {
        if !self.satisfied_by(a.as_ref(), b.as_ref()) {
            return false;
        }
        shrink(a, self.modified_cost_count());
        if let Some(cost) = &self.cost_b {
            shrink(b, cost.count);
        }
        true
    }

    /// `isOutOfStock()`.
    pub fn is_out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }

    /// `needsRestock()`: used at least once.
    pub fn needs_restock(&self) -> bool {
        self.uses > 0
    }

    /// `increaseUses()`.
    pub fn increase_uses(&mut self) {
        self.uses = self.uses.wrapping_add(1);
    }

    /// `resetUses()`.
    pub fn reset_uses(&mut self) {
        self.uses = 0;
    }

    /// `setToOutOfStock()`: what a client's out-of-stock flag reads as.
    pub fn set_to_out_of_stock(&mut self) {
        self.uses = self.max_uses;
    }

    /// `updateDemand()`: up by each use and down by each use left, never clamped
    /// (n10 MerchantOffer 223–238).
    pub fn update_demand(&mut self) {
        self.demand = self
            .demand
            .wrapping_add(self.uses)
            .wrapping_sub(self.max_uses.wrapping_sub(self.uses));
    }

    /// `addToSpecialPriceDiff(d)`.
    pub fn add_to_special_price_diff(&mut self, difference: i32) {
        self.special_price = self.special_price.wrapping_add(difference);
    }

    /// `resetSpecialPriceDiff()`.
    pub fn reset_special_price_diff(&mut self) {
        self.special_price = 0;
    }

    /// `setSpecialPriceDiff(d)`.
    pub fn set_special_price_diff(&mut self, difference: i32) {
        self.special_price = difference;
    }
}

/// `ItemStack.shrink`: a stack left with nothing is the empty stack.
fn shrink(stack: &mut Option<ItemStack>, by: i32) {
    if let Some(held) = stack {
        held.count = held.count.wrapping_sub(by);
        if held.count <= 0 {
            *stack = None;
        }
    }
}

/// `MerchantOffers`: a merchant's offers in the order they were drawn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offers(pub Vec<Offer>);

#[cfg(test)]
mod tests {
    use clustine_data::items;

    use super::*;
    use crate::components::PotionContents;

    const EMERALD: i32 = items::EMERALD;
    const BOOK: i32 = items::BOOK;
    const POTION: i32 = items::POTION;

    fn emeralds(count: i32) -> ItemCost {
        ItemCost {
            item: EMERALD,
            count,
            components: ExactPredicate::EMPTY,
        }
    }

    fn offer(base: i32, multiplier: f32) -> Offer {
        Offer::new(
            emeralds(base),
            None,
            ItemStack::new(items::BREAD, 6),
            16,
            1,
            multiplier,
        )
    }

    fn water() -> Component {
        Component::PotionContents(PotionContents {
            potion: Some("minecraft:water".into()),
            ..PotionContents::default()
        })
    }

    /// Brief 5.4's worked price: base 4, demand 3, multiplier 0.2 and a trading
    /// reputation of 25 (a special price of −5): `4 + floor(12f · 0.2f) − 5`, clamped up
    /// to 1. `12f · 0.2f` is the float nearest 2.4, 2.4000000954, which D10 writes as
    /// 2.4000001.
    #[test]
    fn the_worked_price_of_the_brief_is_one_emerald() {
        let mut offer = offer(4, 0.2);
        offer.demand = 3;
        offer.add_to_special_price_diff(-5);
        assert_eq!(12.0_f32 * 0.2_f32, 2.4_f32);
        assert_eq!(f64::from(2.4_f32), 2.4000000953674316);
        assert_eq!(offer.modified_cost_count(), 1);
        assert_eq!(offer.cost_a(), ItemStack::new(EMERALD, 1));
        // Without the reputation: 4 + 2.
        offer.reset_special_price_diff();
        assert_eq!(offer.modified_cost_count(), 6);
    }

    /// The demand bonus is floored in floats and never below nought: negative demand
    /// takes nothing off, and a price rises at most to a full stack.
    #[test]
    fn negative_demand_takes_nothing_off_and_the_price_stops_at_a_stack() {
        let mut offer = offer(10, 0.05);
        offer.demand = -40;
        assert_eq!(offer.modified_cost_count(), 10);
        offer.demand = 5;
        // floor(50f · 0.05f) = floor(2.5000002) = 2.
        assert_eq!(offer.modified_cost_count(), 12);
        offer.demand = 1000;
        assert_eq!(offer.modified_cost_count(), 64);
        offer.special_price = -100;
        offer.demand = 0;
        assert_eq!(offer.modified_cost_count(), 1);
    }

    /// `updateDemand`: `demand + uses − (maxUses − uses)`, with no clamp either way.
    #[test]
    fn demand_rises_by_uses_and_falls_by_uses_left_without_a_floor() {
        let mut offer = offer(1, 0.05);
        offer.update_demand();
        assert_eq!(offer.demand, -16);
        offer.update_demand();
        assert_eq!(offer.demand, -32);
        for _ in 0..12 {
            offer.increase_uses();
        }
        offer.update_demand();
        assert_eq!(offer.demand, -32 + 12 - 4);
        assert!(!offer.is_out_of_stock());
        assert!(offer.needs_restock());
        offer.increase_uses();
        offer.increase_uses();
        offer.increase_uses();
        offer.increase_uses();
        assert!(offer.is_out_of_stock());
        offer.reset_uses();
        assert!(!offer.is_out_of_stock());
        assert!(!offer.needs_restock());
        offer.set_to_out_of_stock();
        assert_eq!(offer.uses, 16);
    }

    /// `satisfiedBy` and `take`: the first payment at least the modified price, the
    /// second at least its count, or empty where the offer wants none; a stack paid out
    /// whole becomes empty.
    #[test]
    fn an_offer_takes_its_price_from_the_payments_it_is_satisfied_by() {
        let mut book = offer(5, 0.2);
        book.cost_b = Some(ItemCost {
            item: BOOK,
            count: 1,
            components: ExactPredicate::EMPTY,
        });
        let mut a = Some(ItemStack::new(EMERALD, 7));
        let mut b = Some(ItemStack::new(BOOK, 1));
        assert!(!book.satisfied_by(Some(&ItemStack::new(EMERALD, 4)), b.as_ref()));
        assert!(!book.satisfied_by(a.as_ref(), None));
        assert!(!book.satisfied_by(b.as_ref(), a.as_ref()));
        assert!(book.take(&mut a, &mut b));
        assert_eq!(a, Some(ItemStack::new(EMERALD, 2)));
        assert_eq!(b, None);
        assert!(!book.take(&mut a, &mut b));
        assert_eq!(a, Some(ItemStack::new(EMERALD, 2)));

        // Without a second cost the second slot has to be empty.
        let bread = offer(3, 0.05);
        let mut a = Some(ItemStack::new(EMERALD, 3));
        let mut b = Some(ItemStack::new(BOOK, 1));
        assert!(!bread.take(&mut a, &mut b));
        let mut b = None;
        assert!(bread.take(&mut a, &mut b));
        assert_eq!(a, None);
    }

    /// `ItemCost.test` is a subset match: the item, and each component it names equal;
    /// components it does not name, and the count, are not looked at.
    #[test]
    fn a_cost_wants_its_item_and_only_the_components_it_names() {
        let bottle = ItemCost {
            item: POTION,
            count: 1,
            components: ExactPredicate::of(vec![water()]),
        };
        let mut water_bottle = ItemStack::new(POTION, 1);
        water_bottle.components.set(water());
        assert!(bottle.test(&water_bottle));
        water_bottle.components.set(Component::Damage(3));
        assert!(bottle.test(&water_bottle.with_count(9)));
        let mut awkward = ItemStack::new(POTION, 1);
        awkward
            .components
            .set(Component::PotionContents(PotionContents {
                potion: Some("minecraft:awkward".into()),
                ..PotionContents::default()
            }));
        assert!(!bottle.test(&awkward));
        assert!(!bottle.test(&ItemStack::new(POTION, 1)));
        assert!(!bottle.test(&ItemStack::new(BOOK, 1)));
        assert!(!emeralds(1).test(&water_bottle));
        let mut named_emerald = ItemStack::new(EMERALD, 1);
        named_emerald
            .components
            .set(Component::Rarity(crate::components::Rarity::Epic));
        assert!(emeralds(1).test(&named_emerald));
        // The cost's own stack shows what it wants.
        assert_eq!(
            bottle.stack().components.get(water().kind()),
            Some(&water())
        );
    }

    /// Offers read back through serde whole, the multiplier by its bits.
    #[test]
    fn offers_read_back_as_they_were_written() {
        let mut first = offer(4, 0.2);
        first.uses = 3;
        first.demand = -7;
        first.special_price = -2;
        first.cost_b = Some(ItemCost {
            item: POTION,
            count: 1,
            components: ExactPredicate::of(vec![water()]),
        });
        let offers = Offers(vec![first, offer(1, 0.05)]);
        let bytes = postcard::to_allocvec(&offers).expect("offers serialise");
        let back: Offers = postcard::from_bytes(&bytes).expect("offers read back");
        assert_eq!(back, offers);
    }
}
