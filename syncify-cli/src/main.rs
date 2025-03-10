use libsyncify::Syncify;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let _syncify = Syncify::new().await;

    match arg_refs.as_slice() {
        ["send", file] => {
            println!("SEND {}", file);
        }
        ["receive", ticket, file] => println!("RECEIVE from {} {}", ticket, file),
        _ => println!("WTF"),
    }
}
