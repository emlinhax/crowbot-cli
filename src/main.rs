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
mod paths;
mod session;
mod settings;
mod text;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
