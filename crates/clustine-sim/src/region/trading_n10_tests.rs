//! Trading in the region, from the brief's section 5 and `n10-dumps.md`, part B, section
//! 6: a villager's click opens its window with its offers, a trade picked fills the
//! payments, taking the result trades (its price taken, its use counted, its experience
//! and level, its orb), a shift-click trades on until the stock or the payments run out,
//! a close gives the payments back, prices follow reputation and Hero of the Village, a
//! wandering trader trades and lets go of a player who walks off, and a zombie villager
//! cured becomes the villager it was. The worlds are the TNT tests' (`tnt/tests.rs`): a
//! stone floor at y = -61, its top at -60, player 1 at the spawn.

use clustine_data::villagers::professions;
use clustine_data::{entity_types, items};
use clustine_world::trading::{ExactPredicate, ItemCost, Offer, Offers};
use clustine_world::{Entity, EntityData, EntityId, ItemStack, PlayerId};

use super::super::Region;
use super::super::random_ticks::Random;
use super::super::tnt::tests::{add_player, idle, nothing, records, world};
use crate::api::{ClickKind, MenuKind, PlayerEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::body::effects::{self, Effect, Layer};
use crate::mobs::types::gossip::GossipType;
use crate::mobs::types::{villager, wandering_trader, zombie_family};
use crate::state::MenuOf;

const SEED: u64 = 0x5EED_0010;

/// Where player 1 stands, and the merchant two blocks east.
const FEET: [f64; 3] = [0.5, -60.0, 0.5];
const BESIDE: [f64; 3] = [2.5, -60.0, 0.5];

/// The window's slots: the payments, the result, then the main inventory's 27 and the
/// hotbar's 9.
const RESULT: i16 = 2;
const HOTBAR: usize = 30;

fn cost(item: i32, count: i32) -> ItemCost {
    ItemCost {
        item,
        count,
        components: ExactPredicate::EMPTY,
    }
}

/// `count` of `item` for an emerald, `uses` times, `xp` experience, at 0.05.
fn for_an_emerald(item: i32, count: i32, uses: i32, xp: i32) -> Offer {
    Offer::new(
        cost(item, count),
        None,
        ItemStack::new(items::EMERALD, 1),
        uses,
        xp,
        0.05,
    )
}

/// A mob of `kind` made as the region makes one, `dressed` after its making, at
/// `position`, in its chunk; its key.
fn made(
    region: &mut Region,
    kind: i32,
    position: [f64; 3],
    dressed: impl FnOnce(&mut Mob),
) -> EntityId {
    let mut entity = region
        .new_mob(kind as u16, Mob::fresh(), position, 0.0, 0.0)
        .expect("a loaded chunk");
    let mut mob = Mob::decode(&entity.data).expect("a mob");
    dressed(&mut mob);
    entity.data = mob.encode();
    let key = entity.key;
    region.add_made_mob(entity, &mut TickOutput::default());
    key
}

fn record(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .expect("the entity is here")
        .1
        .clone()
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).data).expect("a mob")
}

fn villager_state(region: &Region, key: EntityId) -> villager::VillagerState {
    villager::state_of(&mob(region, key)).1
}

/// A farmer of level 1 with `xp` and `offers` beside player 1, whose hotbar's first slot
/// holds `wheat` wheat.
fn farmer(xp: i32, offers: Vec<Offer>, wheat: i32) -> (Region, PlayerId, EntityId, EntityId) {
    let mut region = world(SEED, nothing);
    let (id, entity) = add_player(&mut region, 1, FEET, false);
    let player = region.players.get_mut(&id).expect("player 1");
    player
        .inventory
        .set(0, (wheat > 0).then(|| ItemStack::new(items::WHEAT, wheat)));
    let key = made(&mut region, entity_types::VILLAGER, BESIDE, |mob| {
        villager::update(mob, |own| {
            own.data.profession = professions::FARMER as u8;
            own.data.level = 1;
            own.xp = xp;
            own.offers = Some(Offers(offers));
        });
    });
    (region, id, entity, key)
}

/// Player 1's click on the merchant `key`, as the input numbered `serial` applies it.
fn use_merchant(
    region: &mut Region,
    id: PlayerId,
    player: EntityId,
    key: EntityId,
    serial: u64,
) -> TickOutput {
    let (chunk, entity) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .expect("the merchant");
    let mut output = TickOutput::default();
    region.applying = Some((id, serial));
    region.mob_used_by(&entity, chunk, player, None, true, false, FEET, &mut output);
    region.applying = None;
    output
}

fn click(region: &mut Region, id: PlayerId, serial: u64, slot: i16, kind: ClickKind) -> TickOutput {
    let mut output = TickOutput::default();
    region.menu_click(id, serial, slot, 0, kind, &mut output);
    output
}

fn select(region: &mut Region, id: PlayerId, serial: u64, index: i32) -> TickOutput {
    let mut output = TickOutput::default();
    region.select_trade(id, serial, index, &mut output);
    output
}

/// The window's own three slots: the payments and the result.
fn window(region: &Region, id: PlayerId) -> [Option<ItemStack>; 3] {
    let trade = region.players[&id]
        .menu
        .as_ref()
        .and_then(|menu| menu.trade.as_deref())
        .expect("a merchant's window");
    [
        trade.payments[0].clone(),
        trade.payments[1].clone(),
        trade.result.clone(),
    ]
}

fn offers_sent(output: &TickOutput) -> Vec<(u64, Offers, i32, i32, bool, bool)> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::MerchantOffers {
                serial,
                offers,
                level,
                xp,
                show_progress,
                can_restock,
            } => Some((
                *serial,
                offers.clone(),
                *level,
                *xp,
                *show_progress,
                *can_restock,
            )),
            _ => None,
        })
        .collect()
}

fn orbs(region: &Region) -> Vec<i32> {
    records(region)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::ExperienceOrb { value, .. } => Some(value),
            _ => None,
        })
        .collect()
}

fn count_of(region: &Region, id: PlayerId, item: i32) -> i32 {
    region.players[&id]
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .map(|stack| stack.count)
        .sum()
}

/// A trade end to end: the click opens the farmer's window (its title by its
/// profession) with its offers, its level and experience; picking the trade fills the
/// first payment with a whole stack of wheat; taking the result pays twenty, counts a
/// use, gives two experience, which reaches level 2 at once (its second set drawn and
/// sent, regeneration, an orb of `3 + nextInt(4) + 5`), and the result is offered again;
/// closing gives the rest of the wheat back and lets the villager go.
#[test]
fn a_trade_opens_the_window_pays_counts_rewards_and_levels_the_villager() {
    let offer = for_an_emerald(items::WHEAT, 20, 16, 2);
    let (mut region, id, player, key) = farmer(8, vec![offer.clone()], 64);
    let opened = use_merchant(&mut region, id, player, key, 41);
    let menu = region.players[&id].menu.clone().expect("a window");
    assert_eq!(menu.of, MenuOf::Merchant(key));
    assert_eq!(menu.kind, MenuKind::Merchant { entity: key });
    let titled: Vec<String> = opened
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::MenuOpened {
                title,
                serial: 41,
                slots,
                ..
            } => {
                assert_eq!(slots.len(), 39);
                Some(title.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(titled, ["entity.minecraft.villager.farmer"]);
    assert_eq!(
        offers_sent(&opened),
        [(41, Offers(vec![offer.clone()]), 1, 8, true, true)]
    );
    assert_eq!(villager_state(&region, key).trading_player, Some(id));

    select(&mut region, id, 41, 0);
    assert_eq!(
        window(&region, id),
        [
            Some(ItemStack::new(items::WHEAT, 64)),
            None,
            Some(ItemStack::new(items::EMERALD, 1)),
        ]
    );
    assert_eq!(count_of(&region, id, items::WHEAT), 0);

    let mut replay = region.entity_stream(key).random;
    let taken = click(&mut region, id, 41, RESULT, ClickKind::Pickup);
    let orb = 3 + replay.next_int(4) as i32 + 5;
    assert_eq!(
        region.players[&id]
            .menu
            .as_ref()
            .and_then(|menu| menu.carried.clone()),
        Some(ItemStack::new(items::EMERALD, 1))
    );
    assert_eq!(
        window(&region, id),
        [
            Some(ItemStack::new(items::WHEAT, 44)),
            None,
            Some(ItemStack::new(items::EMERALD, 1)),
        ]
    );
    let own = villager_state(&region, key);
    assert_eq!((own.data.level, own.xp), (2, 10));
    let offers = own.offers.expect("offers");
    assert_eq!(offers.0[0].uses, 1);
    assert!(offers.0.len() > 1, "the second level's offers added");
    assert_eq!(own.last_traded_player, Some(id));
    assert!(mob(&region, key).body.effects.has(Effect::REGENERATION));
    assert_eq!(orbs(&region), [orb]);
    assert_eq!(offers_sent(&taken), [(41, offers, 2, 10, true, true)]);

    let mut output = TickOutput::default();
    region.close_menu(id, &mut output);
    assert!(region.players[&id].menu.is_none());
    assert_eq!(count_of(&region, id, items::WHEAT), 44);
    assert_eq!(count_of(&region, id, items::EMERALD), 1);
    assert_eq!(villager_state(&region, key).trading_player, None);
}

/// A shift-click on the result trades again while the result comes back: until the
/// offer is out of stock (three uses), or until the payments no longer pay.
#[test]
fn a_shift_click_trades_on_until_the_stock_or_the_payment_runs_out() {
    let offer = for_an_emerald(items::WHEAT, 2, 3, 0);
    let (mut region, id, _, key) = {
        let (mut region, id, player, key) = farmer(0, vec![offer.clone()], 64);
        use_merchant(&mut region, id, player, key, 41);
        (region, id, player, key)
    };
    select(&mut region, id, 41, 0);
    click(&mut region, id, 41, RESULT, ClickKind::QuickMove);
    assert_eq!(count_of(&region, id, items::EMERALD), 3);
    assert_eq!(
        window(&region, id),
        [Some(ItemStack::new(items::WHEAT, 58)), None, None],
        "out of stock: no result"
    );
    let offers = villager_state(&region, key).offers.expect("offers");
    assert!(offers.0[0].is_out_of_stock());
    assert_eq!(orbs(&region).len(), 3);
    // The emeralds went to the hotbar's end first.
    assert_eq!(
        region.players[&id].inventory.0[8],
        Some(ItemStack::new(items::EMERALD, 3))
    );

    // Five wheat at two a trade: two trades, one wheat left.
    let offer = for_an_emerald(items::WHEAT, 2, 10, 0);
    let (mut region, id, player, key) = farmer(0, vec![offer], 5);
    use_merchant(&mut region, id, player, key, 41);
    select(&mut region, id, 41, 0);
    click(&mut region, id, 41, RESULT, ClickKind::QuickMove);
    assert_eq!(count_of(&region, id, items::EMERALD), 2);
    assert_eq!(
        window(&region, id),
        [Some(ItemStack::new(items::WHEAT, 1)), None, None]
    );
    assert_eq!(
        villager_state(&region, key).offers.expect("offers").0[0].uses,
        2
    );
}

/// Picking a trade puts what the payments held back into the player's slots, hotbar end
/// first, then fills the first payment from the player's slots in the window's order
/// (the main inventory before the hotbar) up to a whole stack.
#[test]
fn picking_a_trade_moves_the_payments_back_and_fills_them_from_the_inventory() {
    let wheat = for_an_emerald(items::WHEAT, 20, 16, 2);
    let (mut region, id, player, key) = farmer(0, vec![wheat], 40);
    region
        .players
        .get_mut(&id)
        .expect("player 1")
        .inventory
        .set(9, Some(ItemStack::new(items::WHEAT, 30)));
    use_merchant(&mut region, id, player, key, 41);
    // Five dirt put into the first payment by a click: carried from slot HOTBAR + 4.
    region
        .players
        .get_mut(&id)
        .expect("player 1")
        .inventory
        .set(4, Some(ItemStack::new(items::DIRT, 5)));
    click(&mut region, id, 41, (HOTBAR + 4) as i16, ClickKind::Pickup);
    click(&mut region, id, 41, 0, ClickKind::Pickup);
    assert_eq!(window(&region, id)[0], Some(ItemStack::new(items::DIRT, 5)));
    assert_eq!(window(&region, id)[2], None, "dirt pays for nothing");
    select(&mut region, id, 41, 0);
    let inventory = &region.players[&id].inventory.0;
    assert_eq!(
        inventory[8],
        Some(ItemStack::new(items::DIRT, 5)),
        "the hotbar's end first"
    );
    // 30 from the main inventory's first slot, then 34 of the hotbar's 40.
    assert_eq!(inventory[9], None);
    assert_eq!(inventory[0], Some(ItemStack::new(items::WHEAT, 6)));
    assert_eq!(
        window(&region, id),
        [
            Some(ItemStack::new(items::WHEAT, 64)),
            None,
            Some(ItemStack::new(items::EMERALD, 1)),
        ]
    );
    // A pick of another window's serial, or of a trade that is not one, does nothing.
    select(&mut region, id, 40, 0);
    select(&mut region, id, 41, 5);
    assert_eq!(
        window(&region, id)[0],
        Some(ItemStack::new(items::WHEAT, 64))
    );
}

/// Closing the window gives the payments back to the player and lets the villager go;
/// a window whose player walks out of reach is closed at the end of the tick in the
/// same way.
#[test]
fn a_close_gives_the_payments_back_and_ends_the_trading() {
    let wheat = for_an_emerald(items::WHEAT, 20, 16, 2);
    let (mut region, id, player, key) = farmer(0, vec![wheat.clone()], 64);
    use_merchant(&mut region, id, player, key, 41);
    select(&mut region, id, 41, 0);
    assert_eq!(count_of(&region, id, items::WHEAT), 0);
    let mut output = TickOutput::default();
    region.close_menu(id, &mut output);
    assert_eq!(count_of(&region, id, items::WHEAT), 64);
    assert_eq!(
        region.players[&id].inventory.0[0],
        Some(ItemStack::new(items::WHEAT, 64))
    );
    assert_eq!(villager_state(&region, key).trading_player, None);
    assert!(
        output
            .player_events
            .iter()
            .any(|(_, event)| matches!(event, PlayerEvent::InventorySlot { slot: 0, .. }))
    );

    // Walked off: the window closes, the payments come back.
    let (mut region, id, player, key) = farmer(0, vec![wheat], 64);
    use_merchant(&mut region, id, player, key, 41);
    select(&mut region, id, 41, 0);
    let pose = &mut region.players.get_mut(&id).expect("player 1").pose.position;
    pose.x = 40.5;
    let mut output = TickOutput::default();
    region.tick_menus(&mut output);
    assert!(region.players[&id].menu.is_none());
    assert_eq!(count_of(&region, id, items::WHEAT), 64);
    assert_eq!(villager_state(&region, key).trading_player, None);
}

/// Prices at the opening: a trading reputation of 25 takes `floor(25 × 0.2)` off a book
/// of base 4 and demand 3 (brief 5.4: one emerald), and Hero of the Village takes
/// `max(1, floor(0.3 × 4))` more.
#[test]
fn the_window_s_prices_follow_the_player_s_reputation_and_hero_of_the_village() {
    let mut book = Offer::new(
        cost(items::EMERALD, 4),
        Some(cost(items::BOOK, 1)),
        ItemStack::new(items::BOOKSHELF, 1),
        12,
        1,
        0.2,
    );
    book.demand = 3;
    let (mut region, id, player, key) = farmer(0, vec![book], 0);
    let mut dressed = mob(&region, key);
    villager::update(&mut dressed, |own| {
        own.gossips.add(id.0, GossipType::Trading, 25);
    });
    let (chunk, before) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .expect("the villager");
    region.commit_mob(&before, chunk, &dressed, &mut TickOutput::default());
    let opened = use_merchant(&mut region, id, player, key, 41);
    let sent = offers_sent(&opened);
    let offer = &sent[0].1.0[0];
    assert_eq!(offer.special_price, -5);
    assert_eq!(offer.cost_a().count, 1);
    let mut output = TickOutput::default();
    region.close_menu(id, &mut output);

    let now = region.clock.game_time;
    region.players.get_mut(&id).expect("player 1").effects.add(
        Effect::HERO_OF_THE_VILLAGE,
        Layer::new(0, 24_000, now),
        now,
    );
    let opened = use_merchant(&mut region, id, player, key, 42);
    let sent = offers_sent(&opened);
    assert_eq!(sent[0].1.0[0].special_price, -6);
}

/// A wandering trader's click opens its window with its offers drawn, at level 1 with
/// no progress bar and no restock hint; it lets go of a player who walks more than four
/// blocks off (`TradeWithPlayerGoal`'s stop), and the window closes.
#[test]
fn a_wandering_trader_trades_and_lets_go_of_a_player_who_walks_off() {
    let mut region = world(SEED, nothing);
    let (id, player) = add_player(&mut region, 1, FEET, false);
    let key = made(&mut region, entity_types::WANDERING_TRADER, BESIDE, |_| {});
    // Settled on the floor first.
    for _ in 0..5 {
        idle(&mut region);
    }
    let opened = use_merchant(&mut region, id, player, key, 41);
    let sent = offers_sent(&opened);
    assert_eq!(sent.len(), 1);
    let (serial, offers, level, xp, show_progress, can_restock) = sent[0].clone();
    assert_eq!(
        (serial, level, xp, show_progress, can_restock),
        (41, 1, 0, false, false)
    );
    assert!(!offers.0.is_empty());
    let trader = wandering_trader::trading_of(&mob(&region, key));
    assert_eq!(trader.offers, Some(offers));
    assert_eq!(trader.trading_player, Some(id));
    // Its goals are picked every other tick: two rounds of them beside the player.
    for _ in 0..4 {
        idle(&mut region);
    }
    assert!(
        region.players[&id].menu.is_some(),
        "still trading beside it"
    );
    assert_eq!(
        record(&region, key).position,
        BESIDE,
        "held still for the player"
    );
    region
        .players
        .get_mut(&id)
        .expect("player 1")
        .pose
        .position
        .x = 8.5;
    let mut closed = false;
    for _ in 0..20 {
        idle(&mut region);
        if region.players[&id].menu.is_none() {
            closed = true;
            break;
        }
    }
    assert!(closed, "the window closes once the trader lets go");
    assert_eq!(
        wandering_trader::trading_of(&mob(&region, key)).trading_player,
        None
    );
}

/// The cure: a golden apple on a weak zombie villager starts a countdown of
/// `3600 + nextInt(2401)`, its weakness taken off and strength given; at its end it is
/// the villager it was (its data, offers and experience), thankful to the player who
/// cured it (`major_positive` 20 and `minor_positive` 25: a reputation of 125), and
/// nauseous.
#[test]
fn a_cured_zombie_villager_becomes_the_villager_it_was_with_its_offers() {
    let mut region = world(SEED, nothing);
    let (id, player) = add_player(&mut region, 1, FEET, false);
    let offers = Offers(vec![for_an_emerald(items::PAPER, 24, 16, 2)]);
    let kept = offers.clone();
    let key = made(&mut region, entity_types::ZOMBIE_VILLAGER, BESIDE, |mob| {
        zombie_family::update(mob, |state| {
            let mut villager = state.villager.clone().expect("a zombie villager's data");
            villager.profession = professions::LIBRARIAN;
            villager.level = 3;
            villager.finalised = true;
            villager.xp = 80;
            villager.offers = Some(kept);
            state.villager = Some(villager);
        });
        effects::add(
            mob,
            entity_types::ZOMBIE_VILLAGER as u16,
            Effect::WEAKNESS,
            0,
            1200,
            0,
        );
    });
    let mut replay: Random = region.entity_stream(key).random;
    let (chunk, entity) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .expect("the zombie villager");
    let mut output = TickOutput::default();
    region.mob_used_by(
        &entity,
        chunk,
        player,
        Some(ItemStack::new(items::GOLDEN_APPLE, 1)),
        true,
        false,
        FEET,
        &mut output,
    );
    let time = 3600 + replay.next_int(2401) as i32;
    let zombie = mob(&region, key);
    let state = zombie_family::read(&zombie).villager.expect("its data");
    assert_eq!(state.conversion, time);
    assert_eq!(state.conversion_starter, Some(id.0.as_u128()));
    assert!(!zombie.body.effects.has(Effect::WEAKNESS));
    assert!(zombie.body.effects.has(Effect::STRENGTH));

    // Its countdown near its end.
    let mut near = zombie.clone();
    zombie_family::update(&mut near, |state| {
        if let Some(villager) = state.villager.as_mut() {
            villager.conversion = 1;
        }
    });
    let before = record(&region, key);
    region.commit_mob(&before, chunk, &near, &mut TickOutput::default());
    idle(&mut region);
    assert!(
        region.find_entity(key).is_none(),
        "the zombie villager is gone"
    );
    let cured: Vec<Entity> = records(&region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == entity_types::VILLAGER)
        .collect();
    let [cured] = &cured[..] else {
        panic!("one villager: {cured:?}");
    };
    let villager_mob = Mob::decode(&cured.data).expect("a mob");
    let own = villager::state_of(&villager_mob).1;
    assert_eq!(own.data.profession, professions::LIBRARIAN as u8);
    assert_eq!(own.data.level, 3);
    assert!(own.finalized);
    assert_eq!(own.xp, 80);
    assert_eq!(own.offers, Some(offers));
    assert_eq!(own.gossips.reputation(id.0), 125);
    let nausea = Effect::named("minecraft:nausea").expect("nausea");
    assert!(villager_mob.body.effects.has(nausea));
}
