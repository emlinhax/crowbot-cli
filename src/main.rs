mod agent;
mod api;
mod app;
mod auth;
mod cli;
mod commands;
mod config;
mod conversation;
mod data;
mod effort;
mod forums;
mod frontend;
mod io;
mod live;
mod mode;
mod models;
mod permission;
mod release;
mod session;
#[cfg(test)]
mod testing;
mod text;
mod tools;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
