// The TUI (M3 step 3.5) is the first caller of the renderers; until then only tests use them.
#[cfg_attr(not(test), allow(dead_code))]
pub mod diff;
#[cfg_attr(not(test), allow(dead_code))]
pub mod highlight;
#[cfg_attr(not(test), allow(dead_code))]
pub mod markdown;
pub mod shorten;
#[cfg_attr(not(test), allow(dead_code))]
pub mod styled;
pub mod template;
#[cfg_attr(not(test), allow(dead_code))]
pub mod theme;
pub mod units;
