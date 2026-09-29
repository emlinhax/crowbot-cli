//! The interactive terminal UI.

mod app;
mod boxed;
mod cards;
mod choice;
mod editor;
mod feed;
mod frame;
mod input;
mod keymap;
mod layout;
mod login;
mod palette;
mod queue;
mod screen;
mod status;
mod ui;
mod view;
mod welcome;

pub use app::run;
