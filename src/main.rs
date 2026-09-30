mod agent;
mod api;
mod app;
mod auth;
mod cli;
mod commands;
mod effort;
mod frontend;
mod io;
mod limits;
mod mode;
mod paths;
mod permission;
mod session;
mod settings;
mod text;
mod tools;
mod trust;
mod tui;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
