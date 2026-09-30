//! PixelPlus Imager core (no GUI): everything needed to put a PixelPlus image on an SD
//! card and pre-configure it, shared by the Tauri app and `pixelplus-imager-cli`.
//!
//! * [`settings`]  - the settings form and its `pixelplus.txt` rendering
//! * [`disk`]      - MBR/FAT access; writes `pixelplus.txt` into the boot partition
//! * [`write`]     - streaming (xz) image writer + read-back verification
//! * [`drives`]    - safe removable-drive listing (Linux / macOS / Windows)
//! * [`device`]    - raw device open/unmount/lock per OS
//! * [`job`]       - the privileged write job the helper runs
//! * [`release`]   - GitHub releases / Raspberry Pi Imager repository JSON parsing

pub mod device;
pub mod disk;
pub mod drives;
pub mod job;
pub mod release;
pub mod settings;
pub mod write;

pub use settings::{ImagerSettings, Role};
