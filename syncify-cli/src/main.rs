use crate::parser::Commands;
use fern::colors::ColoredLevelConfig;
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
                ColoredLevelConfig::new().color(record.level()),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .level_for("iroh", log::LevelFilter::Off)
        .level_for("iroh_net_report", log::LevelFilter::Off)
        .level_for("portmapper", log::LevelFilter::Off)
        .level_for("zbus", log::LevelFilter::Off)
        .level_for("tracing", log::LevelFilter::Off)
        .level_for("swarm_discovery", log::LevelFilter::Off)
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}
