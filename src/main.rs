mod agent;
mod api;
mod app;
mod auth;
mod cli;
mod commands;
mod effort;
mod frontend;
mod install;
mod io;
mod limits;
mod mode;
mod paths;
mod permission;
mod release;
mod session;
mod settings;
mod text;
mod tools;
mod trust;
mod tui;
mod update;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
