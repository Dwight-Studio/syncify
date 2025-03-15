use crate::parser::Commands;

mod parser;

#[tokio::main]
async fn main() {
    Commands::run().await;
}
