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

**What a player does to another region's blocks is passed on to that region.** A player
reaches a few blocks beyond the stripe they stand in. Their own region checks what only
it knows, that they are within reach and what they hold, and passes the rest on through
the player's edge to the region that has the block. Placing can need both regions: the
one that has the block that was clicked says whether there is something to place
against, and the one that has the spot next to it says whether it is free. The region
that takes the last step reports the action as done, after the tick's changes.

The edge tells a client that an action was handled only when no earlier action of that
player is still on its way to another region. A client shows its own guess of what an
action did until it is told that the action was handled, and what the server said from
then on; told too early, it would show the old block again for a moment.

This was added a day after the rest (2026-10-08). The first version judged every action
in the player's region, which does not have the chunks beyond its stripe, so blocks
across a boundary could not be changed. Trying it with real clients showed that to be
a seam nobody would accept, also for a milestone.

## Alternatives considered

- **Workers hand players to each other directly.** Saves the detour, but the edge has to
  switch where it sends inputs at exactly the right moment anyway, and learning of the
  hand-over from one of the two workers is the same race with one more party.
- **The old region returns the inputs it could not apply.** Needs no numbers, but the
  returned inputs arrive after newer ones that were already sent to the new region, so
  either the order is lost or the edge has to hold inputs back until the old region
  confirms it has nothing more to return, which delays every hand-over by a tick.
- **Leaving blocks across a boundary out of reach.** Needs no path for actions between
  regions, and with dynamic regions a boundary will never run where players can reach
  it. But until then every boundary is a line that building stops at.
- **Regions keeping a copy of their neighbours' border chunks and players**, so that a
  player's region can judge everything itself and only the write goes elsewhere. Closest
  to what one region does, and what halo exchange would need anyway, but far more than
  a boundary that is going to disappear is worth.

## Consequences

- With one edge, everything the new region says about a player is caused by the edge
  having read the old region's last word on them, so reports from two regions about one
  entity cannot arrive in the wrong order. With several edges they can, for the edges
  that only watch; versions on entity states will be needed then.
- A player standing on a boundary can be handed back and forth every tick.
- An action on another region's blocks takes effect a tick or two later than one on
  the player's own. Two actions on the same block in quick succession, one that needs
  two regions and one that does not, can therefore take effect in the other order than
  they were made in.
- A block is not placed where a player of the region that has the spot stands, nor
  where the one who places it stands. A player of the other region who stands astride
  the boundary, overlapping the spot, is not seen and can be built into by a few tenths
  of a block.
- A hand-over costs the others a tick or two in which the player does not move for them.
- If a region is so far behind that the edge no longer keeps what it missed, the player
  is disconnected rather than left with a state the server does not share.
- Numbered inputs that can be sent again are also what moving a whole region to another
  worker needs, so M3 builds on them.
