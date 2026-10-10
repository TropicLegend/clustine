//! Two connections as one player: a second login as a name that is still connected.
//!
//! A server lets one of the two stay. Which, and what the other is told, is what this
//! shows: both bots are kept, each with what it was sent and how its connection ended
//! ([`Bot::report`]).

use std::time::Duration;

use anyhow::{Context, Result};

use crate::{Behaviour, Bot, Ending, Entry};

/// What two logins as one name came to.
pub struct SameName {
    /// The bot that was connected first, with everything it was sent.
    pub first: Bot,
    /// How the first connection ended, or `None` if it was still there when the
    /// patience ran out.
    pub first_end: Option<Ending>,
    /// What became of the second login: entered, refused with a reason, or failed
    /// without one.
    pub second: Result<Entry>,
}

/// Joins `address` as `name`, and joins again as the same name while the first
/// connection is there. Both connections are attended to at the same time, so neither
/// waits for the other's packets to be read.
///
/// Where the first connection is not ended this takes all of `patience`, during which
/// the second bot answers nothing: keep it well under the 30 s after which a server
/// gives up on a client that does not answer its keep-alives.
pub async fn same_name_twice(address: &str, name: &str, patience: Duration) -> Result<SameName> {
    let mut first = Bot::join(address, name).await.context("the first login")?;
    let (first_end, second) = tokio::join!(
        first.wait_for_end(patience),
        Bot::attempt(address, name, Behaviour::default())
    );
    Ok(SameName {
        first,
        first_end,
        second,
    })
}
