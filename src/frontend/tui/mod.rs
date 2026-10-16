//! The interactive terminal UI.

mod app;
mod bar;
mod boxed;
mod calls;
mod card;
mod choice;
mod command;
mod editor;
mod feed;
mod forums;
mod keymap;
mod layout;
mod login;
mod palette;
mod paste;
mod picker;
mod progress;
mod questions;
mod queue;
mod replace;
mod rows;
mod screen;
mod signup;
mod toast;
mod ui;
mod view;
mod welcome;

pub use app::run;
