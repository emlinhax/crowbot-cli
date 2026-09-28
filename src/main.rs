mod api;
mod app;
mod cli;
mod commands;
mod io;
mod limits;
mod paths;
mod settings;
mod text;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
