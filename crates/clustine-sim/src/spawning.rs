//! Natural spawning's rules (`docs/groundwork/n9-plan.md`, sections 9.3 and 9.4): what
//! the per-chunk spawner (`region/natural_spawning.rs`) asks of a position and a type,
//! apart from the region, so that each rule is a function of what it reads. The frame,
//! the counts and the caps, is the region's (`region/spawn_caps.rs`).

pub(crate) mod rules;
