use crate::logger::SyncifyLogger;
use libsyncify::Syncify;
use log::debug;
use spdlog::Level;

mod logger;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let logger = SyncifyLogger::new();
    logger.set_max_level(Level::Info);

    let mut syncify = Syncify::new().await.unwrap();

    match arg_refs.as_slice() {
        ["sync"] => {
            syncify.start_sync().await.unwrap();
            //let dir = syncify.create_shared_directory(PathBuf::from("target/debug/examples")).await.unwrap();
            //let dir = syncify.create_shared_directory(PathBuf::from("target/debug/download")).await.unwrap();

            //info!("Created shared dir with sign_key = {} and verif_key = {}", dir.sign_key(), dir.verif_key());
        }
        ["reset"] => {}
        ["help"] => {}
        _ => debug!("WTF"),
    }

    //syncify.stop_sync().await;
}
