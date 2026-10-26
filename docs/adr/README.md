# Decision records

Why Clustine is the way it is, one decision a record. A record keeps its text as it
was agreed; its status line says whether it is built and what has replaced it since.
Where a later record changes an earlier one, the later one says so, and the earlier
one names it. As of 2026-10-26:

| Record | Title | Status |
|---|---|---|
| [0001](0001-language.md) | Implementation language | Accepted |
| [0002](0002-licence.md) | Licence | Accepted |
| [0003](0003-partitioning.md) | World partitioning model | Accepted |
| [0004](0004-game-data.md) | Generated game data is committed | Accepted; amended by 0019 |
| [0005](0005-edge-worker-interface.md) | The edge forwards semantic inputs, not packets | Accepted |
| [0006](0006-static-regions-and-handoff.md) | Fixed regions and handing players over through the edge | M2; the stripes replaced in M3 (0010, 0017), the hand-over kept |
| [0007](0007-coordinator-scope.md) | A single coordinator without storage, for now | M2; mostly superseded in M3, the single coordinator in memory still holds |
| [0008](0008-durable-regions-and-resuming.md) | Durable regions, output commit and resuming | Built (M3, phase A) |
| [0009](0009-moving-a-region.md) | Moving a region on purpose | Built (M3, phase B) |
| [0010](0010-regions-that-follow-players.md) | Regions that follow players | Built (M3, phase C, steps C0 to C6) |
| [0011](0011-the-world-store-and-regions.md) | The world store and regions | Built (step C1) |
| [0012](0012-the-tick-on-chunks.md) | The tick on chunks | Built (step C2b) |
| [0013](0013-the-edge-without-a-layout.md) | The edge without a layout | Built (the edge's part of C2b) |
| [0014](0014-merging-and-splitting.md) | Merging and splitting | Built (step C3) |
| [0015](0015-the-edge-through-merges-and-splits.md) | The edge through merges and splits | Built (the edge's part of C3) |
| [0016](0016-when-to-merge-and-split.md) | When to merge and when to split | Built (step C4) |
| [0017](0017-the-end-of-the-stripes.md) | The end of the stripes | Built (step C5) |
| [0018](0018-a-checkpoints-chunks-written-together.md) | A checkpoint's chunks are written together | Built (step C5.10) |
| [0019](0019-data-made-from-mojangs-jar.md) | Data made from Mojang's jar, and the notices for what is ported | Built as far as terrain needs it; the templates' blocks from the operator's jar not yet |
| [0020](0020-one-stay-per-player.md) | One stay per player, and a player's place kept by the world store | Built (replicas, phase R1) |
| [0021](0021-generation-is-a-function.md) | Generation is a function, decorated in a fixed order | Built (W5); no feature runs yet |
| [0022](0022-what-a-chunk-carries.md) | What a chunk carries, and where light is made | Built (steps C1 to C5) |
| [0023](0023-light-at-the-edge.md) | Light at the edge | Built (L0a to L7, which is W3); L8 not built |
| [0024](0024-block-behaviour-in-a-region.md) | Block behaviour in a region | Built (phase B, B1 to B8) |
| [0025](0025-effects-between-regions.md) | Effects between regions | Built and measured (phase E, E1 to E7) |
| [0026](0026-redstone.md) | Redstone | Accepted; R2 to R7 built, R8 to come (phase R) |
| [0027](0027-lending-at-a-line.md) | Lending land at a pinned line, and merging at once at a busy one | Accepted; L2 to L5 built, L6 to come |
| [0028](0028-entities.md) | Entities other than players: keys, chunks, commits and crossing | Accepted; built through phase N |
| [0029](0029-light-in-the-simulation.md) | Light in the simulation | Proposed; N8a in part built (the engine's interface with its first implementation, the noting, the step and the budget) |

Drafts that are not records yet are in [`../groundwork`](../groundwork).
