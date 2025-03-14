use crate::logger::SyncifyLogger;
use crate::parser::Commands;
use spdlog::Level;

mod logger;
mod parser;

#[tokio::main]
async fn main() {
    let logger = SyncifyLogger::new();
    logger.set_max_level(Level::Warn);

    Commands::run().await;
}
