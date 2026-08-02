//! Library surface for buzz-cli's provider clients, so buzz-gateway can
//! reuse the exact same implementations instead of a second, drifting
//! copy. main.rs is a thin binary on top of this — its own `mod theme;`
//! (terminal-only) stays private to the binary.

pub mod providers;
