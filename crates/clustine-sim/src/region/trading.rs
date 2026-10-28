//! Trading with a villager or a wandering trader in the region (`AbstractVillager`,
//! `Villager.startTrading`, `MerchantMenu`, `MerchantContainer`, `MerchantResultSlot`;
//! `docs/groundwork/n10-plan.md`, section 8.2, with `n10-dumps.md`, part B, section 6):
//! opening the merchant's window, a trade picked, the payments' result, a trade made by
//! taking the result, and the end of trading; and the end of a zombie villager's cure,
//! the villager it becomes given back what it was.
//!
//! The merchant's rules are `crate::trading::merchant`'s; this side reads the merchant
//! from its record, draws from its stream aside (a player's use and a window's clicks
//! come between its ticks), and commits it.
//!
//! Named differences: a merchant's window shows the type's name, not a custom name; the
//! payments' result is worked out once after a click, not at each slot the click set
//! (what a client sees after the click is the same; a yes or no the merchant says at a
//! payment set between two others of one click is said once for the click's last); the
//! offers are sent again after a trade only where the villager reached a level (the
//! trade's reputation event a tick later is part V3's); a cured villager's preserved
//! equipment (`dropPreservedEquipment`) is neither dropped nor kept.

use clustine_data::entity_types;
use clustine_world::{Entity, EntityId, ItemStack, PlayerId};

use super::Region;
use super::entities::Thrown;
use super::random_ticks::Random;
use crate::api::{MenuKind, PlayerEvent, RegionEvent, TickOutput};
use crate::menus::{self, MerchantSide};
use crate::mobs::Mob;
use crate::mobs::body::effects::{self, Effect};
use crate::mobs::body::ladder;
use crate::mobs::types::gossip::GossipType;
use crate::mobs::types::{self, villager, zombie_villager};
use crate::state::{Menu, MenuOf, TradeSlots};
use crate::trading::merchant;

/// The squared reach within which a merchant's window stays open
/// (`isWithinEntityInteractionRange(merchant, 4)`: a creative player's 5 and 4 more,
/// from the eyes to the merchant's box), as the region keeps a vehicle's.
const STILL_VALID: f64 = 9.0 * 9.0;

/// A merchant's side of a click of its window (`MerchantResultSlot`): the merchant as
/// its record had it, changed by the trades the click makes, and what they did beyond
/// it, for the region to carry out after the click.
pub(super) struct MerchantTrade {
    key: EntityId,
    kind: u16,
    mob: Mob,
    before: Mob,
    moment: u32,
    position: [f64; 3],
    hint: i32,
    player: PlayerId,
    reputation: i32,
    hero: Option<u8>,
    stream: Random,
    start: Random,
    orbs: Vec<i32>,
    said: Vec<(i32, f32)>,
    levelled: bool,
}

impl MerchantTrade {
    /// `updateSellItem` of the payments of `container`: its result set, and the merchant
    /// told (`notifyTradeUpdated`).
    fn update_sell_item(&mut self, container: &mut [Option<ItemStack>]) {
        let offers = merchant::offers(&self.mob, self.kind).unwrap_or_default();
        let payments = payments_of(container);
        let sold = merchant::sell_item(&offers, &payments, self.hint);
        if let Some(place) = container.get_mut(2) {
            *place = sold.result.clone();
        }
        if sold.notify
            && let Some(said) = merchant::notify_trade_updated(
                &mut self.mob,
                self.kind,
                self.moment,
                sold.result.is_some(),
                &mut self.stream,
            )
        {
            self.said.push(said);
        }
    }
}

impl MerchantSide for MerchantTrade {
    fn may_pickup(&self, container: &[Option<ItemStack>]) -> bool {
        let offers = merchant::offers(&self.mob, self.kind).unwrap_or_default();
        merchant::may_take(&offers, &payments_of(container), self.hint)
    }

    /// `MerchantResultSlot.onTake` (n10 MerchantResultSlot 120–177): the active offer
    /// takes its price from the payments as they are, else the other way round; where it
    /// does, the merchant is told of the trade, and the payments are set again, each
    /// working the result out anew.
    fn on_take(&mut self, container: &mut [Option<ItemStack>]) {
        let offers = merchant::offers(&self.mob, self.kind).unwrap_or_default();
        let payments = payments_of(container);
        let Some(at) = merchant::sell_item(&offers, &payments, self.hint).active else {
            return;
        };
        let offer = &offers.0[at];
        let [mut a, mut b] = payments;
        if !(offer.take(&mut a, &mut b) || offer.take(&mut b, &mut a)) {
            return;
        }
        let traded = merchant::notify_trade(
            &mut self.mob,
            self.kind,
            at,
            &mut self.stream,
            self.moment,
            Some(self.player),
            self.reputation,
            self.hero,
        );
        self.orbs.extend(traded.orb);
        self.levelled |= traded.levelled;
        if let Some(place) = container.first_mut() {
            *place = a;
        }
        self.update_sell_item(container);
        if let Some(place) = container.get_mut(1) {
            *place = b;
        }
        self.update_sell_item(container);
    }
}

/// The two payments of a merchant's window's container.
fn payments_of(container: &[Option<ItemStack>]) -> [Option<ItemStack>; 2] {
    [
        container.first().cloned().flatten(),
        container.get(1).cloned().flatten(),
    ]
}

impl Region {
    /// The merchant `key` as its record has it, if it is a merchant here: its chunk, its
    /// record and itself.
    fn merchant_at(&self, key: EntityId) -> Option<(clustine_world::ChunkPos, Entity, Mob)> {
        let (chunk, entity) = self.find_entity(key)?;
        if !merchant::is_merchant(entity.kind) {
            return None;
        }
        let mob = Mob::decode(&entity.data).ok()?;
        Some((chunk, entity.clone(), mob))
    }

    /// What a villager's prices read of the player `player`: the reputation the villager
    /// `mob` gives them (`getPlayerReputation`, every gossip about them), and their Hero
    /// of the Village's amplifier.
    fn trading_facts(&self, player: PlayerId, mob: &Mob, kind: u16) -> (i32, Option<u8>) {
        if !merchant::is_villager(kind) {
            return (0, None);
        }
        let reputation = villager::state_of(mob).1.gossips.reputation(player.0);
        let hero = self
            .players
            .get(&player)
            .and_then(|own| own.effects.amplifier(Effect::HERO_OF_THE_VILLAGE));
        (reputation, hero)
    }

    /// Tells the player `player` of the merchant's offers in their window of the serial
    /// `serial` (`sendMerchantOffers`).
    fn send_offers(
        &self,
        player: PlayerId,
        serial: u64,
        mob: &Mob,
        kind: u16,
        output: &mut TickOutput,
    ) {
        let offers = merchant::offers(mob, kind).unwrap_or_default();
        let (level, xp, show_progress, can_restock) = merchant::window_facts(mob, kind);
        output.player_events.push((
            player,
            PlayerEvent::MerchantOffers {
                serial,
                offers,
                level,
                xp,
                show_progress,
                can_restock,
            },
        ));
    }

    /// The merchant's voice at its feet, heard by everyone.
    fn merchant_says(
        &self,
        kind: u16,
        position: [f64; 3],
        said: (i32, f32),
        output: &mut TickOutput,
    ) {
        output.events.push(RegionEvent::Sound {
            sound: said.0,
            source: ladder::sound_source(kind),
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            volume: types::of(kind).sound_volume(),
            pitch: said.1,
            except: None,
        });
    }

    /// `startTrading(player)`: the merchant `merchant` (a villager or a wandering trader)
    /// opens its window for the player `player`, who used it. A villager's prices are
    /// worked out for them first (`updateSpecialPrices`, while it trades with nobody, so
    /// that nothing is sent); then it trades with them, their window opens
    /// (`openTradingScreen`: `MerchantMenu`, menu type 19) and, where it has offers, they
    /// are sent with its level, its experience, and the progress bar and restock hint a
    /// villager shows and a trader does not. Only for the player whose input is being
    /// applied, whose window it can name.
    pub(super) fn start_trading(
        &mut self,
        merchant: EntityId,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        if self.applying.map(|(id, _)| id) != Some(player) {
            return;
        }
        let Some((chunk, entity, mut mob)) = self.merchant_at(merchant) else {
            return;
        };
        let kind = entity.kind;
        if merchant::is_villager(kind) {
            let (reputation, hero) = self.trading_facts(player, &mob, kind);
            let mut offers = merchant::offers(&mob, kind).unwrap_or_default();
            merchant::update_special_prices(&mut offers, reputation, hero);
            merchant::set_offers(&mut mob, kind, Some(offers));
        }
        merchant::set_trading_player(&mut mob, kind, Some(player));
        self.commit_mob(&entity, chunk, &mob, output);
        self.open_menu(
            player,
            MenuOf::Merchant(merchant),
            MenuKind::Merchant { entity: merchant },
            output,
        );
        let serial = self
            .players
            .get(&player)
            .and_then(|own| own.menu.as_ref())
            .filter(|menu| menu.of == MenuOf::Merchant(merchant))
            .map(|menu| menu.serial);
        let Some(serial) = serial else {
            return;
        };
        if merchant::offers(&mob, kind).is_some_and(|offers| !offers.0.is_empty()) {
            self.send_offers(player, serial, &mob, kind, output);
        }
    }

    /// The merchant's side of a click of the player's window `menu`, if it is a
    /// merchant's: the merchant read, its stream aside, and what the player's prices
    /// read of them.
    pub(super) fn merchant_side(&self, player: PlayerId, menu: &Menu) -> Option<MerchantTrade> {
        let MenuOf::Merchant(key) = menu.of else {
            return None;
        };
        let (_, entity, mob) = self.merchant_at(key)?;
        let (reputation, hero) = self.trading_facts(player, &mob, entity.kind);
        let stream = self.entity_stream(key).random;
        Some(MerchantTrade {
            key,
            kind: entity.kind,
            before: mob.clone(),
            mob,
            moment: entity.lived,
            position: entity.position,
            hint: menu
                .trade
                .as_ref()
                .and_then(|trade| trade.selected)
                .unwrap_or(0),
            player,
            reputation,
            hero,
            start: stream.clone(),
            stream,
            orbs: Vec::new(),
            said: Vec::new(),
            levelled: false,
        })
    }

    /// What a click's trades did: the merchant committed where it changed, its draws
    /// counted, its voice heard, its orbs dropped, and, where a villager reached a level,
    /// its offers sent to the player (`updateTrades`' `updateSpecialPrices`).
    pub(super) fn merchant_traded(
        &mut self,
        player: PlayerId,
        trade: MerchantTrade,
        output: &mut TickOutput,
    ) {
        self.entity_drew(trade.key, trade.stream.draws_since(&trade.start));
        if trade.mob != trade.before
            && let Some((chunk, entity)) = self.find_entity(trade.key)
        {
            let entity = entity.clone();
            self.commit_mob(&entity, chunk, &trade.mob, output);
        }
        for said in &trade.said {
            self.merchant_says(trade.kind, trade.position, *said, output);
        }
        let [x, y, z] = trade.position;
        for value in &trade.orbs {
            self.add_orb([x, y + 0.5, z], *value, output);
        }
        if trade.levelled
            && let Some(serial) = self
                .players
                .get(&player)
                .and_then(|own| own.menu.as_ref())
                .map(|menu| menu.serial)
        {
            self.send_offers(player, serial, &trade.mob, trade.kind, output);
        }
    }

    /// `updateSpecialPrices(tradingPlayer)` of the merchant `merchant` (Villager
    /// 417–440): its prices for the player it trades with worked out again; where their
    /// window is open on it, the payments' result worked out again (`updateSellItem`)
    /// and the offers sent them as they stand.
    pub(super) fn prices_again(&mut self, merchant: EntityId, output: &mut TickOutput) {
        let Some((chunk, entity, mut mob)) = self.merchant_at(merchant) else {
            return;
        };
        let kind = entity.kind;
        let Some(player) = merchant::trading_player(&mob, kind) else {
            return;
        };
        let (reputation, hero) = self.trading_facts(player, &mob, kind);
        let mut offers = merchant::offers(&mob, kind).unwrap_or_default();
        if offers.0.is_empty() {
            return;
        }
        merchant::update_special_prices(&mut offers, reputation, hero);
        merchant::set_offers(&mut mob, kind, Some(offers));
        self.commit_mob(&entity, chunk, &mob, output);
        let serial = self
            .players
            .get(&player)
            .and_then(|own| own.menu.as_ref())
            .filter(|menu| menu.of == MenuOf::Merchant(merchant))
            .map(|menu| menu.serial);
        let Some(serial) = serial else {
            return;
        };
        self.merchant_payments_changed(player, merchant, output);
        if let Some((_, entity, mob)) = self.merchant_at(merchant) {
            self.send_offers(player, serial, &mob, entity.kind, output);
        }
    }

    /// `handleSelectTrade` (n6 ServerGamePacketListenerImpl 2568–2610): in the player's
    /// merchant window of the serial `serial`, while it is valid, the trade `index`
    /// becomes the selection hint (the result worked out again), and the payments are
    /// moved for it (`tryMoveItems`). Refused for any other window.
    pub(super) fn select_trade(
        &mut self,
        player: PlayerId,
        serial: u64,
        index: i32,
        output: &mut TickOutput,
    ) {
        let Some(menu) = self.players.get(&player).and_then(|own| own.menu.clone()) else {
            return;
        };
        let MenuOf::Merchant(key) = menu.of else {
            return;
        };
        if menu.serial != serial || menu.trade.is_none() {
            return;
        }
        let Some(own) = self.players.get(&player) else {
            return;
        };
        let feet = own.pose.position;
        let eyes = [feet.x, feet.y + 1.62, feet.z];
        if !self.merchant_still_valid(key, eyes) {
            return;
        }
        let Some(mut trade) = self.merchant_side(player, &menu) else {
            return;
        };
        trade.hint = index;
        let Some(own) = self.players.get_mut(&player) else {
            return;
        };
        let Some(slots) = own.menu.as_mut().and_then(|menu| menu.trade.as_deref_mut()) else {
            return;
        };
        slots.selected = Some(index);
        let mut container = vec![
            slots.payments[0].clone(),
            slots.payments[1].clone(),
            slots.result.clone(),
        ];
        let mut inventory = own.inventory.clone();
        // `setSelectionHint`: the result for the hint.
        trade.update_sell_item(&mut container);
        try_move_items(&mut trade, &mut container, &mut inventory, index);
        let Some(own) = self.players.get_mut(&player) else {
            return;
        };
        own.inventory = inventory;
        if let Some(slots) = own.menu.as_mut().and_then(|menu| menu.trade.as_deref_mut()) {
            slots.payments = payments_of(&container);
            slots.result = container.get(2).cloned().flatten();
        }
        self.journal.players.insert(player);
        self.merchant_traded(player, trade, output);
    }

    /// `MerchantContainer.updateSellItem`: the payments of the player's merchant window
    /// changed; its result is worked out again for them, and the merchant told.
    pub(super) fn merchant_payments_changed(
        &mut self,
        player: PlayerId,
        merchant: EntityId,
        output: &mut TickOutput,
    ) {
        let Some(menu) = self.players.get(&player).and_then(|own| own.menu.clone()) else {
            return;
        };
        if menu.of != MenuOf::Merchant(merchant) {
            return;
        }
        let Some(slots) = menu.trade.as_deref() else {
            return;
        };
        let mut container = vec![
            slots.payments[0].clone(),
            slots.payments[1].clone(),
            slots.result.clone(),
        ];
        let Some(mut trade) = self.merchant_side(player, &menu) else {
            return;
        };
        trade.update_sell_item(&mut container);
        if let Some(slots) = self
            .players
            .get_mut(&player)
            .and_then(|own| own.menu.as_mut())
            .and_then(|menu| menu.trade.as_deref_mut())
        {
            slots.result = container.get(2).cloned().flatten();
        }
        self.journal.players.insert(player);
        self.merchant_traded(player, trade, output);
    }

    /// `AbstractVillager.stillValid(player)` (575–592) of the merchant `merchant` for the
    /// player whose eyes are at `eyes`: the merchant here and alive, trading with a player
    /// of the region whose window is on it, and within their reach. A villager with no
    /// offers stops trading (`customServerAiStep`'s `stopTrading`), so its window is not
    /// valid either.
    pub(super) fn merchant_still_valid(&self, merchant: EntityId, eyes: [f64; 3]) -> bool {
        let Some((_, entity, mob)) = self.merchant_at(merchant) else {
            return false;
        };
        if mob.body.vitals.dying() {
            return false;
        }
        let trading = merchant::trading_player(&mob, entity.kind).is_some_and(|player| {
            self.players
                .get(&player)
                .and_then(|own| own.menu.as_ref())
                .is_some_and(|menu| menu.of == MenuOf::Merchant(merchant))
        });
        if !trading {
            return false;
        }
        if merchant::is_villager(entity.kind)
            && merchant::offers(&mob, entity.kind).is_none_or(|offers| offers.0.is_empty())
        {
            return false;
        }
        super::uses::distance_to_sqr(self.entity_box(&entity), eyes) < STILL_VALID
    }

    /// The translation key of the merchant's window's title (`getDisplayName`): a
    /// villager's by its profession, a trader's by its type.
    pub(super) fn merchant_title(&self, merchant: EntityId) -> String {
        match self.merchant_at(merchant) {
            Some((_, entity, mob)) => merchant::title(&mob, entity.kind),
            None => "entity.minecraft.villager".to_owned(),
        }
    }

    /// The merchant `merchant` trades with nobody where it traded with `player`
    /// (`setTradingPlayer(null)`).
    fn stop_trading(&mut self, player: PlayerId, merchant: EntityId, output: &mut TickOutput) {
        let Some((chunk, entity, mut mob)) = self.merchant_at(merchant) else {
            return;
        };
        if merchant::trading_player(&mob, entity.kind) != Some(player) {
            return;
        }
        merchant::set_trading_player(&mut mob, entity.kind, None);
        self.commit_mob(&entity, chunk, &mob, output);
    }

    /// The player's merchant window on `merchant` closed, for whatever reason, the player
    /// staying: the merchant trades with nobody (`setTradingPlayer(null)`), and the
    /// payments go back to them, the first then the second, as a closed window's carried
    /// stack does (`MerchantMenu.removed`'s `placeItemBackInInventory`), what does not fit
    /// thrown.
    pub(super) fn trading_closed(
        &mut self,
        player: PlayerId,
        merchant: EntityId,
        trade: &mut TradeSlots,
        output: &mut TickOutput,
    ) {
        self.stop_trading(player, merchant, output);
        for payment in &mut trade.payments {
            let Some(stack) = payment.take().filter(|stack| stack.count > 0) else {
                continue;
            };
            let Some(own) = self.players.get_mut(&player) else {
                return;
            };
            let mut holder = Menu {
                serial: 0,
                of: MenuOf::Merchant(merchant),
                kind: MenuKind::Merchant { entity: merchant },
                carried: Some(stack),
                drag: None,
                trade: None,
            };
            let selected = usize::from(own.selected_slot);
            if let Some(left) = menus::put_back(&mut own.inventory, selected, &mut holder) {
                self.throw(
                    Thrown {
                        player,
                        stack: left,
                        crouching: false,
                    },
                    output,
                );
            }
        }
        trade.result = None;
        self.journal.players.insert(player);
    }

    /// The player leaves the world with a merchant window on `merchant` open: the
    /// merchant trades with nobody, and the payments are thrown where `throw` (a player
    /// removed drops them, `MerchantMenu.removed`).
    pub(super) fn trading_left(
        &mut self,
        player: PlayerId,
        merchant: EntityId,
        trade: Option<&TradeSlots>,
        throw: bool,
        output: &mut TickOutput,
    ) {
        self.stop_trading(player, merchant, output);
        if !throw {
            return;
        }
        let payments = trade
            .map(|trade| trade.payments.clone())
            .unwrap_or_default();
        for stack in payments
            .into_iter()
            .flatten()
            .filter(|stack| stack.count > 0)
        {
            self.throw(
                Thrown {
                    player,
                    stack,
                    crouching: false,
                },
                output,
            );
        }
    }

    /// Whether the player of the UUID `uuid` is in the world (`getPlayerByUUID`): the
    /// region's, a face it keeps, or one of the world's players the store published.
    fn player_in_world(&self, uuid: u128) -> bool {
        let id = PlayerId(uuid::Uuid::from_u128(uuid));
        self.players.contains_key(&id)
            || self.faces.kept.contains_key(&id)
            || self
                .faces
                .world
                .iter()
                .flatten()
                .any(|known| known.player == id)
    }

    /// The end of a zombie villager's cure (`ZombieVillager.finishConversion`'s
    /// `finalizeConversion`, n7 ZombieVillager 737–852), in the conversion of the region's
    /// mob `old` of the record `record` into the mob `new` of the type `into`, after what
    /// every conversion copies and before the villager is added: it is given the
    /// zombie's villager data and whether that is settled, its gossips, its offers and
    /// its experience; it is finalised as one converted (`finalizeSpawn(CONVERSION)`,
    /// the level's draws from the zombie's stream aside, the mob's from its own `own`);
    /// its brain is made again (`refreshBrain`, its sensors' delays drawn of `own`); the
    /// player who began the cure, where they are in the world, is thanked
    /// (`ZOMBIE_VILLAGER_CURED`: `major_positive` 20 and `minor_positive` 25 about them);
    /// and it is nauseous for 200 ticks. Nothing for any other conversion.
    pub(super) fn cure_finished(
        &mut self,
        record: &Entity,
        old: &Mob,
        into: u16,
        new: &mut Mob,
        own: &mut Random,
    ) {
        if i32::from(record.kind) != entity_types::ZOMBIE_VILLAGER
            || i32::from(into) != entity_types::VILLAGER
        {
            return;
        }
        let zombie = zombie_villager::villager(old);
        villager::update(new, |villager| {
            villager.finalized = zombie.finalised;
            villager.data = villager::Data {
                kind: u8::try_from(zombie.kind).unwrap_or(0),
                profession: u8::try_from(zombie.profession).unwrap_or(0),
                level: zombie.level,
            };
            villager.gossips = zombie.gossips.clone();
            villager.offers = zombie.offers.clone();
            villager.xp = zombie.xp;
        });
        let block = crate::physics::containing(record.position);
        let biome = self.biome_at(block);
        let day = self.clock.day;
        let game_time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let mut level = self.entity_stream(record.key).random;
        let start = level.clone();
        let mut group = None;
        crate::mobs::making::finalize(
            into,
            &mut types::Spawning {
                mob: new,
                level: &mut level,
                own,
                biome,
                at: block,
                day,
                game_time,
                reason: types::SpawnReason::Conversion,
                group: &mut group,
                structure: None,
                moon_brightness: crate::mobs::making::moon_brightness(day),
                difficulty: (0.0, 0.0),
                companions: None,
            },
        );
        self.entity_drew(record.key, level.draws_since(&start));
        let brain = crate::mobs::brain::made(&villager::PROVIDER, 0, own);
        crate::mobs::part::put(&mut new.mind.brain, &brain);
        if let Some(starter) = zombie.conversion_starter
            && self.player_in_world(starter)
        {
            let about = uuid::Uuid::from_u128(starter);
            villager::update(new, |villager| {
                villager.gossips.add(about, GossipType::MajorPositive, 20);
                villager.gossips.add(about, GossipType::MinorPositive, 25);
            });
        }
        if let Some(nausea) = Effect::named("minecraft:nausea") {
            effects::add(new, into, nausea, 0, 200, 0);
        }
    }
}

/// `MerchantMenu.tryMoveItems(index)` (n10 MerchantMenu 410–490) of the window's
/// container `container` (the payments then the result) over the player's `inventory`:
/// nothing for an index that names no offer; each payment that is not empty goes back
/// into the player's slots, hotbar end first (`moveItemStackTo(3, 39, reverse)`), the
/// method ending where nothing of it moves and keeping what did not; then, where both
/// payments are empty, the first is filled for the offer's first cost and the second for
/// its second, each to a whole stack from the player's slots in order
/// (`moveFromInventoryToPaymentSlot`). Each payment set works the result out again.
fn try_move_items(
    trade: &mut MerchantTrade,
    container: &mut [Option<ItemStack>],
    inventory: &mut clustine_world::Inventory,
    index: i32,
) {
    let offers = merchant::offers(&trade.mob, trade.kind).unwrap_or_default();
    let Some(offer) = usize::try_from(index)
        .ok()
        .and_then(|at| offers.0.get(at))
        .cloned()
    else {
        return;
    };
    let kind = MenuKind::Merchant { entity: trade.key };
    let mut slots: Vec<Option<ItemStack>> = (3..kind.slots())
        .map(|index| {
            menus::inventory_slot_of(kind, index)
                .and_then(|slot| inventory.get(slot).flatten())
                .filter(|stack| stack.count > 0)
        })
        .collect();
    let mut stopped = false;
    for payment in 0..2 {
        let Some(mut stack) = container[payment].clone().filter(|stack| stack.count > 0) else {
            continue;
        };
        if !move_into(&mut slots, &mut stack, true) {
            stopped = true;
            break;
        }
        container[payment] = Some(stack).filter(|stack| stack.count > 0);
        trade.update_sell_item(container);
    }
    if !stopped && container[0].is_none() && container[1].is_none() {
        fill_payment(trade, container, &mut slots, 0, &offer.cost_a);
        if let Some(cost) = &offer.cost_b {
            fill_payment(trade, container, &mut slots, 1, cost);
        }
    }
    for (at, stack) in slots.into_iter().enumerate() {
        if let Some(slot) = menus::inventory_slot_of(kind, at + 3) {
            inventory.set(slot, stack);
        }
    }
}

/// `moveFromInventoryToPaymentSlot(payment, cost)`: each of the player's slots in order
/// that holds what the cost wants (and the payment's own item, if it holds one) gives
/// what the payment has room for, up to a whole stack; the payment set each time.
fn fill_payment(
    trade: &mut MerchantTrade,
    container: &mut [Option<ItemStack>],
    slots: &mut [Option<ItemStack>],
    payment: usize,
    cost: &clustine_world::trading::ItemCost,
) {
    for slot in slots.iter_mut() {
        let Some(stack) = slot.as_mut().filter(|stack| stack.count > 0) else {
            continue;
        };
        if !cost.test(stack) {
            continue;
        }
        let paid = container[payment].clone().filter(|paid| paid.count > 0);
        if paid.as_ref().is_some_and(|paid| !paid.same_kind(stack)) {
            continue;
        }
        let most = crate::physics::item::max_stack_size(stack);
        let held = paid.map_or(0, |paid| paid.count);
        let moved = (most - held).min(stack.count);
        let now = stack.with_count(held + moved);
        stack.count -= moved;
        if stack.count <= 0 {
            *slot = None;
        }
        let full = now.count >= most;
        container[payment] = Some(now);
        trade.update_sell_item(container);
        if full {
            break;
        }
    }
}

/// `moveItemStackTo(stack, 3, 39, reverse)` over the player's slots of a window
/// (`slots`, the main inventory's then the hotbar's): first into the stacks of the same
/// item, then all that is left into the first empty slot, both in that order (last
/// first when `reverse`). Whether anything moved.
fn move_into(slots: &mut [Option<ItemStack>], stack: &mut ItemStack, reverse: bool) -> bool {
    let order: Vec<usize> = if reverse {
        (0..slots.len()).rev().collect()
    } else {
        (0..slots.len()).collect()
    };
    // A slot of the player's holds 99 at most, or the item's own most if less.
    let most = crate::physics::item::max_stack_size(stack).min(99);
    let mut moved = false;
    if most > 1 {
        for &at in &order {
            if stack.count <= 0 {
                break;
            }
            let Some(here) = slots[at].as_mut() else {
                continue;
            };
            if !here.same_kind(stack) {
                continue;
            }
            let total = here.count + stack.count;
            if total <= most {
                stack.count = 0;
                here.count = total;
                moved = true;
            } else if here.count < most {
                stack.count -= most - here.count;
                here.count = most;
                moved = true;
            }
        }
    }
    if stack.count > 0
        && let Some(&at) = order.iter().find(|&&at| slots[at].is_none())
    {
        let part = stack.count.min(most);
        slots[at] = Some(stack.with_count(part));
        stack.count -= part;
        moved = true;
    }
    moved
}

#[cfg(test)]
#[path = "trading_n10_tests.rs"]
mod trading_n10_tests;
