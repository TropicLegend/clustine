//! A merchant's window at the edge (`docs/groundwork/n10-plan.md`, sections 8.2 and 11):
//! its offers as `merchant_offers` for the window the player's menu of a serial is shown
//! in.
//!
//! An offer goes as `MerchantOffer.writeToStream` writes it (n10 MerchantOffer 437–549,
//! `docs/groundwork/n10-dumps.md`, part B, section 7): the first cost as its base, before
//! the price is modified, for the client works the price out itself from the demand, the
//! special price and the multiplier sent beside it; each cost with the components its
//! payment has to have; out of stock as `uses >= maxUses`.

use bytes::Bytes;
use clustine_protocol::packets;
use clustine_protocol::packets::play::{MerchantOffers, WireItemCost, WireOffer};
use clustine_world::trading::{ItemCost, Offer, Offers};

use crate::menus::Menus;

/// What the region's offers of the menu `serial` are sent as: `merchant_offers` for the
/// window that shows it, or nothing where no such window is shown.
pub(crate) fn offers(
    menus: &Menus,
    serial: u64,
    offers: &Offers,
    level: i32,
    xp: i32,
    show_progress: bool,
    can_restock: bool,
) -> Option<Bytes> {
    let window_id = menus.window_of(serial)?;
    let packet = MerchantOffers {
        window_id,
        offers: offers.0.iter().map(wire_offer).collect(),
        level,
        xp,
        show_progress,
        can_restock,
    };
    Some(packets::encode(&packet).into())
}

fn wire_offer(offer: &Offer) -> WireOffer {
    WireOffer {
        cost_a: wire_cost(&offer.cost_a),
        // The game writes the result whole, an empty one as a count of nothing, as the
        // protocol writes a stack of no count.
        result: Some(crate::encode::wire_stack(&offer.result)),
        cost_b: offer.cost_b.as_ref().map(wire_cost),
        out_of_stock: offer.uses >= offer.max_uses,
        uses: offer.uses,
        max_uses: offer.max_uses,
        xp: offer.xp,
        special_price: offer.special_price,
        price_multiplier: offer.price_multiplier.0,
        demand: offer.demand,
    }
}

/// `ItemCost.STREAM_CODEC`: the components the payment has to have are the predicate's,
/// each with its kind's id and value, in the predicate's order (one of a kind, in the
/// kinds' order, which its patch keeps).
fn wire_cost(cost: &ItemCost) -> WireItemCost {
    WireItemCost {
        item: cost.item,
        count: cost.count,
        components: crate::stack_wire::wire_components(&cost.components.as_patch()).added,
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::items;
    use clustine_protocol::components::{WireComponent, potion};
    use clustine_protocol::packets::play::ClientboundPlay;
    use clustine_sim::api::MenuKind;
    use clustine_world::components::PotionContents;
    use clustine_world::trading::ExactPredicate;
    use clustine_world::{Component, EntityId, ItemStack};

    use super::*;

    fn cost(item: i32, count: i32) -> ItemCost {
        ItemCost {
            item,
            count,
            components: ExactPredicate::EMPTY,
        }
    }

    fn bread_for_emeralds() -> Offer {
        Offer::new(
            cost(items::EMERALD, 1),
            None,
            ItemStack::new(items::BREAD, 6),
            16,
            1,
            0.05,
        )
    }

    /// Menus showing a merchant's window for the menu `serial`.
    fn showing(serial: u64) -> Menus {
        let mut menus = Menus::default();
        let kind = MenuKind::Merchant {
            entity: EntityId(7),
        };
        menus.opened(
            serial,
            serial,
            kind,
            "entity.minecraft.villager.farmer",
            vec![None; kind.slots()],
            None,
        );
        menus
    }

    fn read(bytes: &Bytes) -> MerchantOffers {
        match ClientboundPlay::decode(bytes).expect("a packet the edge writes") {
            ClientboundPlay::MerchantOffers(packet) => packet,
            other => panic!("not merchant_offers: {other:?}"),
        }
    }

    #[test]
    fn offers_go_to_the_window_that_shows_their_menu() {
        let menus = showing(5);
        let bytes = offers(
            &menus,
            5,
            &Offers(vec![bread_for_emeralds()]),
            2,
            15,
            true,
            false,
        )
        .expect("the window is shown");
        let packet = read(&bytes);
        assert_eq!(packet.window_id, menus.window_of(5).expect("shown"));
        assert_eq!(packet.level, 2);
        assert_eq!(packet.xp, 15);
        assert!(packet.show_progress);
        assert!(!packet.can_restock);
        assert_eq!(packet.offers.len(), 1);
    }

    #[test]
    fn offers_of_a_menu_not_shown_are_not_sent() {
        let menus = showing(5);
        let offers_of = |serial| {
            offers(
                &menus,
                serial,
                &Offers(vec![bread_for_emeralds()]),
                1,
                0,
                true,
                true,
            )
        };
        assert!(offers_of(4).is_none());
        assert!(offers_of(6).is_none());
        assert!(offers(&Menus::default(), 5, &Offers(Vec::new()), 1, 0, true, true).is_none());
    }

    #[test]
    fn an_offer_is_written_with_four_byte_integers_after_its_stacks() {
        let mut offer = bread_for_emeralds();
        offer.uses = 3;
        offer.special_price = -2;
        offer.demand = 5;
        let menus = showing(1);
        let bytes = offers(&menus, 1, &Offers(vec![offer]), 1, 0, true, true).expect("shown");
        let mut expected = Vec::new();
        expected.push(
            u8::try_from(clustine_protocol::packet_ids::play::clientbound::MERCHANT_OFFERS)
                .expect("one byte"),
        );
        // The window (the first, 1) and one offer.
        expected.extend([1, 1]);
        // Cost A: the emerald (1013 as a VarInt), one, no components.
        expected.extend([0xF5, 0x07, 1, 0]);
        // The result: six of bread (1067), no components added or removed.
        expected.extend([6, 0xAB, 0x08, 0, 0]);
        // No cost B, in stock.
        expected.extend([0, 0]);
        expected.extend(3i32.to_be_bytes());
        expected.extend(16i32.to_be_bytes());
        expected.extend(1i32.to_be_bytes());
        expected.extend((-2i32).to_be_bytes());
        expected.extend(0.05f32.to_be_bytes());
        expected.extend(5i32.to_be_bytes());
        // The level, the experience, the progress shown, restocking.
        expected.extend([1, 0, 1, 1]);
        assert_eq!(&bytes[..], &expected[..]);
    }

    #[test]
    fn the_first_cost_goes_as_its_base_and_the_second_as_it_is() {
        let mut offer = Offer::new(
            cost(items::EMERALD, 10),
            Some(cost(items::BOOK, 1)),
            ItemStack::new(items::ENCHANTED_BOOK, 1),
            12,
            5,
            0.2,
        );
        offer.demand = 4;
        offer.special_price = 3;
        let packet =
            read(&offers(&showing(1), 1, &Offers(vec![offer]), 1, 0, true, true).expect("shown"));
        let wire = &packet.offers[0];
        assert_eq!(wire.cost_a.item, items::EMERALD);
        assert_eq!(wire.cost_a.count, 10);
        let cost_b = wire.cost_b.as_ref().expect("a second cost");
        assert_eq!((cost_b.item, cost_b.count), (items::BOOK, 1));
        assert_eq!(wire.demand, 4);
        assert_eq!(wire.special_price, 3);
        assert_eq!(wire.price_multiplier.to_bits(), 0.2f32.to_bits());
        assert_eq!(wire.xp, 5);
    }

    #[test]
    fn an_offer_used_up_is_out_of_stock() {
        let mut used_up = bread_for_emeralds();
        used_up.uses = 16;
        let mut beyond = bread_for_emeralds();
        beyond.uses = 17;
        let mut nearly = bread_for_emeralds();
        nearly.uses = 15;
        let packet = read(
            &offers(
                &showing(1),
                1,
                &Offers(vec![used_up, beyond, nearly]),
                1,
                0,
                true,
                true,
            )
            .expect("shown"),
        );
        let flags: Vec<bool> = packet.offers.iter().map(|o| o.out_of_stock).collect();
        assert_eq!(flags, [true, true, false]);
        assert_eq!(packet.offers[1].uses, 17);
    }

    #[test]
    fn a_cost_names_the_components_its_payment_has_to_have() {
        let water = PotionContents {
            potion: Some("minecraft:water".into()),
            custom_colour: None,
            custom_effects: Vec::new(),
            custom_name: None,
        };
        let offer = Offer::new(
            ItemCost {
                item: items::POTION,
                count: 1,
                components: ExactPredicate::of(vec![Component::PotionContents(water)]),
            },
            None,
            ItemStack::new(items::EMERALD, 1),
            12,
            1,
            0.05,
        );
        let packet =
            read(&offers(&showing(1), 1, &Offers(vec![offer]), 1, 0, true, true).expect("shown"));
        let [WireComponent::PotionContents(contents)] = &packet.offers[0].cost_a.components[..]
        else {
            panic!("the potion's contents only");
        };
        let water_id = potion::NAMES
            .iter()
            .position(|name| *name == "minecraft:water")
            .expect("water is a potion");
        assert_eq!(
            contents.potion,
            Some(i32::try_from(water_id).expect("a small id"))
        );
        assert!(contents.custom_effects.is_empty());
    }
}
