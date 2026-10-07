//! The interactive terminal UI.

mod app;
mod boxed;
mod card;
mod cards;
mod choice;
mod command;
mod editor;
mod feed;
mod forums;
mod frame;
mod keymap;
mod layout;
mod login;
mod palette;
mod paste;
mod picker;
mod questions;
mod queue;
mod screen;
mod status;
mod toast;
mod ui;
mod view;
mod welcome;

pub use app::run;
