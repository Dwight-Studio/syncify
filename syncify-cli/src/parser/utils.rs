/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

use colored::Colorize;
use fern::colors::ColoredLevelConfig;
use std::collections::HashMap;
use std::fmt::Display;
use std::io::Write;
use std::time::SystemTime;

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
        .level(log::LevelFilter::Debug)
        .level_for("iroh", log::LevelFilter::Off)
        .level_for("iroh_quinn", log::LevelFilter::Off)
        .level_for("iroh_gossip", log::LevelFilter::Off)
        .level_for("iroh_relay", log::LevelFilter::Off)
        .level_for("iroh_net_report", log::LevelFilter::Off)
        .level_for("iroh_quinn_proto", log::LevelFilter::Off)
        .level_for("events.net.relay.connected", log::LevelFilter::Off)
        .level_for("hyper_util", log::LevelFilter::Off)
        .level_for("acto", log::LevelFilter::Off)
        .level_for("portmapper", log::LevelFilter::Off)
        .level_for("zbus", log::LevelFilter::Off)
        .level_for("tracing", log::LevelFilter::Off)
        .level_for("swarm_discovery", log::LevelFilter::Off)
        .level_for("rustls", log::LevelFilter::Off)
        .level_for("hickory_proto", log::LevelFilter::Off)
        .level_for("reqwest", log::LevelFilter::Off)
        .level_for("hickory_resolver", log::LevelFilter::Off)
        .level_for("igd_next", log::LevelFilter::Off)
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}

pub async fn number_choice<T: Display>(vec: Vec<T>, head_message: Option<&str>) -> usize {
    loop {
        print_clear();
        if let Some(msg) = head_message {
            println!("{msg}\n")
        }
        for (i, dir) in vec.iter().enumerate() {
            println!("{}> {}", i + 1, dir);
        }

        print_no_newline("\nChoice> ".purple().bold());

        if let Ok(ch) = read_input().trim_end().parse::<usize>() {
            if ch > 0 && ch - 1 < vec.len() {
                break ch - 1;
            }
        }
    }
}

pub async fn hash_choice<T: Display, U: Clone>(map: HashMap<T, U>, head_message: Option<&str>) -> U {
    loop {
        print_clear();
        if let Some(msg) = head_message {
            println!("{msg}\n")
        }
        for (i, dir) in map.keys().enumerate() {
            println!("{}> {}", i + 1, dir);
        }

        print_no_newline("\nChoice> ".purple().bold());

        if let Ok(ch) = read_input().trim_end().parse::<usize>() {
            if ch > 0 && ch - 1 < map.len() {
                let vec: Vec<U> = map.values().cloned().collect();
                break vec[ch - 1].clone();
            }
        }
    }
}

pub fn print_no_newline<T: Display>(msg: T) {
    print!("{msg}");
    std::io::stdout().flush().unwrap();
}

pub fn read_input() -> String {
    let choice = &mut String::new();
    std::io::stdin().read_line(choice).unwrap();

    choice.to_string()
}

pub fn print_clear() {
    print!("\x1B[2J\x1B[1;1H");
    std::io::stdout().flush().unwrap();
}

pub fn print_error<T: Display>(msg: T) {
    println!("{}{}{}", "error:".red().bold(), " ".normal(), msg)
}

pub fn print_success<T: Display>(msg: T) {
    println!("{}{}{}", "\nsuccess:".green().bold(), " ".normal(), msg)
}
