//! PixelPlus core: show model, fseq, xLights import, mapping, effects,
//! scheduling and power estimation. Pure logic, no I/O besides files.

pub mod model;

// Data path (owned by the "core-data" workstream)
pub mod fseq;
pub mod ppseq;
pub mod xlights;
pub mod mapping;
pub mod power;
pub mod layout;

// Show logic (owned by the "core-show" workstream)
pub mod effects;
pub mod schedule;
pub mod sun;
pub mod faultfinder;
pub mod template;
pub mod text;
