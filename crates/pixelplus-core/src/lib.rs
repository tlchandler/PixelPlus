//! PixelPlus core: show model, fseq, xLights import, mapping, effects,
//! scheduling and power estimation. Pure logic, no I/O besides files.

pub mod model;

// Data path (owned by the "core-data" workstream)
pub mod fseq;
pub mod layout;
pub mod mapping;
pub mod power;
pub mod ppseq;
pub mod xlights;

// Show logic (owned by the "core-show" workstream)
pub mod effects;
pub mod faultfinder;
pub mod schedule;
pub mod sun;
pub mod template;
pub mod text;

// Feature wave (docs/ARCHITECTURE.md §12). Created by WS0; each file has one owner.
pub mod audio_analysis; // WS2 (F2)
pub mod autoshow; // WS2 (F2)
pub mod calpattern; // WS1 (F1)
pub mod mapcode; // WS4 (F6/F7)
pub mod preview; // WS2 (F3)
pub mod smartlist; // WS2 (F18)
