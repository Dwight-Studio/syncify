mod logger;

use libsyncify::Syncify;
use crate::logger::SyncifyLogger;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    SyncifyLogger::init();

    let mut syncify = Syncify::new().await.unwrap();

    match arg_refs.as_slice() {
        ["start"] => {
            syncify.start_sync().await.unwrap();
        }
        ["send", file] => {
            println!("SEND {}", file);
        }
        ["receive", ticket, file] => println!("RECEIVE from {} {}", ticket, file),
        _ => println!("WTF"),
    }
    
    syncify.stop_sync().await;
}
