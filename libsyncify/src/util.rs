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
use blake3::Hash;
use chrono::{DateTime, Utc};
use ed25519_dalek::Signature;
use rkyv::{Archive, Deserialize, Serialize};
use std::time::SystemTime;
use fern::colors::ColoredLevelConfig;

#[derive(Archive, Serialize, Deserialize)]
#[rkyv(remote = blake3::Hash)]
#[rkyv(archived = ArchivedHash)]
pub struct HashDef(#[rkyv(getter = blake3::Hash::as_bytes)] [u8; 32]);

impl From<HashDef> for Hash {
    fn from(hash_def: HashDef) -> Self {
        Hash::from_bytes(hash_def.0)
    }
}

#[derive(Archive, Serialize, Deserialize)]
#[rkyv(remote = DateTime::<Utc>)]
#[rkyv(archived = ArchivedDateTime)]
pub struct DateTimeDef(#[rkyv(getter = DateTime::timestamp)] i64);

impl From<DateTimeDef> for DateTime<Utc> {
    fn from(date_time_def: DateTimeDef) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(date_time_def.0, 0).unwrap()
    }
}

#[derive(Archive, Serialize, Deserialize)]
#[rkyv(remote = Option::<Hash>)]
#[rkyv(archived = ArchivedOptionHash)]
pub enum OptionHashDef {
    Some(#[rkyv(with = HashDef)] Hash),
    None,
}

impl From<OptionHashDef> for Option<Hash> {
    fn from(hash_def: OptionHashDef) -> Option<Hash> {
        match hash_def {
            OptionHashDef::Some(hash) => Option::Some(hash),
            OptionHashDef::None => Option::None,
        }
    }
}

#[derive(Archive, Serialize, Deserialize)]
#[rkyv(remote = ed25519_dalek::Signature)]
#[rkyv(archived = ArchivedSignature)]
pub struct SignatureDef(#[rkyv(getter = Signature::to_bytes)] [u8; 64]);

impl From<SignatureDef> for Signature {
    fn from(signature_def: SignatureDef) -> Signature {
        Signature::from_bytes(&signature_def.0)
    }
}

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
        .level_for("relm4", log::LevelFilter::Off)
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}