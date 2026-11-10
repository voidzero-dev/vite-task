//! Scripted runs played into `vt_tui`: in the browser page in `web/`
//! ([`Demo`], built for `wasm32-unknown-unknown` and driven from JavaScript),
//! and in a real terminal by the `vt_tui_demo` binary ([`Player`], or
//! [`Plain`] when stdout isn't a terminal).

mod names;
mod plain;
mod player;
mod scenarios;
mod sim;
mod web;

pub use names::RunNames;
pub use plain::Plain;
pub use player::Player;
pub use scenarios::{Scenario, dev, verify};
pub use web::Demo;
