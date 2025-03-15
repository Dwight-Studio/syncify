use std::collections::HashMap;
use std::fmt::Display;
use std::io::Write;
use std::time::SystemTime;
use fern::colors::ColoredLevelConfig;

pub fn setup_logger() -> Result<(), fern::InitError> {
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

pub async fn number_choice<T: Display>(vec: Vec<T>, head_message: Option<&str>, ) -> usize {
    loop {
        print!("\x1B[2J\x1B[1;1H");
        std::io::stdout().flush().unwrap();
        if let Some(msg) = head_message { println!("{msg}\n") }
        for (i, dir) in vec.iter().enumerate() {
            println!("{}> {}", i + 1, dir);
        }

        print!("\nChoice> ");
        std::io::stdout().flush().unwrap();

        let choice = &mut String::new();
        std::io::stdin().read_line(choice).unwrap();

        if let Ok(ch) = choice.trim_end().parse::<usize>() {
            if ch > 0 && ch - 1 < vec.len() {
                break ch - 1
            }
        }
    }
}

pub async fn hash_choice<T: Display, U: Clone>(map: HashMap<T, U>, head_message: Option<&str>, ) -> U {
    loop {
        print!("\x1B[2J\x1B[1;1H");
        std::io::stdout().flush().unwrap();
        if let Some(msg) = head_message { println!("{msg}\n") }
        for (i, dir) in map.keys().enumerate() {
            println!("{}> {}", i + 1, dir);
        }

        print!("\nChoice> ");
        std::io::stdout().flush().unwrap();

        let choice = &mut String::new();
        std::io::stdin().read_line(choice).unwrap();

        if let Ok(ch) = choice.trim_end().parse::<usize>() {
            if ch > 0 && ch - 1 < map.len() {
                let vec: Vec<U> = map.values().cloned().collect();
                break vec[ch - 1].clone()
            }
        }
    }
}