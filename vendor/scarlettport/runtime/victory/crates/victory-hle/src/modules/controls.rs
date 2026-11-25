//! Keyboard and mouse played the way PC games are. The logic (key layouts,
//! the mouse as a right stick, latching, merging with pads and scripted
//! input) is platform-neutral and lives in `victory_core::kbm`, where it is
//! unit-tested on every host; `gameinput` feeds it from the host's input.

pub use victory_core::kbm::{
    Controls, Counts, MOUSE_BUTTONS, Settings, configure, key_code, settings,
};
