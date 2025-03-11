mod logger;

use crate::logger::SyncifyLogger;
use libsyncify::Syncify;
use log::{debug, error, info};
use spdlog::{Level};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

   SyncifyLogger::new();

    let mut syncify = Syncify::new().await.unwrap();

    match arg_refs.as_slice() {
        ["start"] => {
            syncify.start_sync().await.unwrap();
        }
        ["send", file] => {
            debug!("SEND {}", file);
        }
        ["receive", ticket, file] => debug!("RECEIVE from {} {}", ticket, file),
        _ => debug!("WTF"),
    }

    syncify.stop_sync().await;
}
