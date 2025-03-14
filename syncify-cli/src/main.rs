use crate::parser::Commands;
use std::time::SystemTime;

mod parser;

#[tokio::main]
async fn main() {
    //let logger = SyncifyLogger::new();
    //logger.set_max_level(Level::Warn);

    setup_logger().unwrap();

    Commands::run().await;
}

fn setup_logger() -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                humantime::format_rfc3339_seconds(SystemTime::now()),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .level_for("iroh", log::LevelFilter::Off)
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}
