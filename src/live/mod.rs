//! What a run shares with its tools and its frontend while it runs: the events it reports, the
//! prompts that wait on the user, the state both sides change, and what the model has read.
//! The stored conversation is session/'s.

pub mod event;
pub mod files;
pub mod prompt;
pub mod state;
