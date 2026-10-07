# ADR-0006: Fixed regions and handing players over through the edge

- Status: **Accepted** for milestone M2; the fixed regions are replaced in M3
- Date: 2026-10-07

## Context

M2 is the first milestone in which more than one worker simulates the world. It has to
prove the pieces that every later milestone builds on: an edge that shows one world out
of what several workers publish, and a player who moves from one worker to another
without their client noticing. It does not have to solve where the boundaries between
workers should run; [ADR-0003](0003-partitioning.md) leaves that to dynamic regions.

## Decision

**Regions are fixed stripes.** The world is divided along the x axis at chunk coordinates
given when the cluster is started. A region loads only chunks of its stripe and holds
only players who stand in it.

**A player is handed over through their edge, keeping their entity.**

1. The region whose stripe a player walks out of removes them and tells their edge, with
   everything it knows about them. It reports the last move as usual and does not report
   the entity as removed.
2. The edge passes the player on to the region they walked into and from then on sends
   what they do there.
3. That region takes the player in with the same entity id and reports the entity as it
   reports any that appears. An edge that already shows it treats that as an update.

Whoever watches sees the entity walk across. The player's client is told nothing at all.

**Inputs are numbered and sent again.** What a player does goes to the region the edge
believes them to be in, which may have let them go a moment ago and ignores it. The edge
therefore numbers a player's inputs and keeps the latest ones. A region reports the
number of the last input it applied when it lets a player go, the edge sends everything
after that to the next region, and a region ignores numbers it has applied already. A
region also stops applying a player's inputs within a tick once one of them has taken
the player out of its stripe, so that what the player did next is judged by the region
they are in.

**What a region is sent keeps its order.** A tick applies changes of players before
inputs, so an input that reached a region while the player was away must not be applied
once the player is back, ahead of earlier ones that are sent again. A change of a player
therefore drops what that player did before it and is still waiting.

**The edge is the one place that knows where a player is.** All inputs go through its
fan-out task, which also performs the hand-over in one uninterrupted step.

**A player who leaves during a hand-over is cleaned up after by entity id.** The region
they were in ignores the leave, and the hand-over then arrives for a player the edge no
longer has, or has again as another entity. The edge sees that the entity does not match
and has the region the entity was heading for report it as removed. Besides, an edge
hides the entity of a leaving player at once and never shows a client two entities of
one player.

**Blocks across a boundary are out of reach.** A player's actions are judged by the
region the player is in, which does not have the chunks beyond its stripe.

## Alternatives considered

- **Workers hand players to each other directly.** Saves the detour, but the edge has to
  switch where it sends inputs at exactly the right moment anyway, and learning of the
  hand-over from one of the two workers is the same race with one more party.
- **The old region returns the inputs it could not apply.** Needs no numbers, but the
  returned inputs arrive after newer ones that were already sent to the new region, so
  either the order is lost or the edge has to hold inputs back until the old region
  confirms it has nothing more to return, which delays every hand-over by a tick.
- **Forwarding block changes to the region that owns the block.** Removes the seam, but
  needs a second path for actions between regions, and placing a block needs to know who
  stands there, which the owning region does not. With dynamic regions a boundary never
  runs where players can reach it, so the path would be built to be removed.

## Consequences

- With one edge, everything the new region says about a player is caused by the edge
  having read the old region's last word on them, so reports from two regions about one
  entity cannot arrive in the wrong order. With several edges they can, for the edges
  that only watch; versions on entity states will be needed then.
- A player standing on a boundary can be handed back and forth every tick.
- A hand-over costs the others a tick or two in which the player does not move for them.
- If a region is so far behind that the edge no longer keeps what it missed, the player
  is disconnected rather than left with a state the server does not share.
- Numbered inputs that can be sent again are also what moving a whole region to another
  worker needs, so M3 builds on them.
