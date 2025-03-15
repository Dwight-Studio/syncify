use std::fmt::Display;
use std::io::Write;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use libsyncify::{SharedDirPermission, Syncify};
use std::path::PathBuf;
use tokio::sync::mpsc;
use uuid::Uuid;
use libsyncify::store::link::Link;

#[derive(Parser)]
#[command(name = "Syncify Command Line Interface")]
#[command(version, about, long_about = None)]
pub(crate) struct Commands {
    #[command(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand)]
enum Subcommands {
    /// Start the synchronisation process
    Sync,
    /// Invite a peer to a shared folder. This command can be run without any arguments.
    Invite {
        /// The UUID of the folder to share
        #[arg(long)]
        uuid: Option<String>,
        /// Permission of the shared folder
        #[arg(value_enum, long, default_value_t = InvitePermission::Write)]
        permission: InvitePermission,
    },
    /// List all shared directory
    List,
    /// Create a shared directory
    Create {
        #[arg(long)]
        path: String,
    },
    /// Remove a shared directory
    Remove {
        #[arg(long)]
        uuid: String,
    },
    /// Join a shared directory
    Join {
        #[arg(long)]
        link: String,
        #[arg(long)]
        path: String,
    },
    /// Reset the configuration and shared folders
    Reset,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum InvitePermission {
    ReadOnly,
    Write,
}

impl Commands {
    pub(crate) async fn run() {
        let cli = Commands::parse();
        let mut cmd = Commands::command();
        let mut syncify = Syncify::new().await.unwrap();

        match &cli.command {
            Subcommands::Sync => {
                syncify.start_sync().await.unwrap();
                syncify.create_shared_directory(PathBuf::from("target/debug/examples")).await.unwrap();
                let (tx, mut rx) = mpsc::channel::<u8>(1);
                ctrlc::set_handler(move || {
                    tx.blocking_send(1).unwrap();
                })
                .unwrap();
                rx.recv().await.unwrap();
                syncify.stop_sync().await;
            }
            Subcommands::Invite { uuid, permission } => {
                if uuid.is_none() {
                    let dirs = syncify.get_all_shared_directories().await;

                    if dirs.is_empty() { cmd.error(ErrorKind::Io, "No shared folder found! Please join or create a shared folder!"); }

                    let choice = Self::number_choice(dirs.iter().map(|dir| dir.path().display().to_string()).collect(), Some("Choose a directory to share:")).await;
                    
                    let uuid = dirs[choice].uuid();
                    let path = dirs[choice].path();
                } else if let Ok(uuid_str) = Uuid::parse_str(uuid.clone().unwrap().as_str()) {
                    match *permission {
                        InvitePermission::ReadOnly => {
                            println!(
                                "Link: {}",
                                Link::builder(syncify)
                                    .dir_uuid(uuid_str)
                                    .permission(SharedDirPermission::ReadOnly)
                                    .build().await.unwrap()
                            );
                        }
                        InvitePermission::Write => {
                            if let Ok(link) = Link::builder(syncify)
                                .dir_uuid(uuid_str)
                                .build().await {
                                println!(
                                    "Link: {link}",
                                )
                            } else {
                                cmd.error(ErrorKind::Io, "You cannot share a directory with read-only access as a directory with write permission!");
                            }
                        }
                    }
                } else {
                    cmd.error(
                        ErrorKind::InvalidValue,
                        format!("Invalid UUID: {}", uuid.clone().unwrap()),
                    );
                }
            }
            Subcommands::List => {
                for dir in syncify.get_all_shared_directories().await {
                    println!("'{}' at '{}'", dir.path().display(), dir.uuid())
                }
            }
            Subcommands::Create { path } => {
                match path.parse::<PathBuf>() {
                    Ok(pathbuf) => {
                        match syncify
                            .create_shared_directory(pathbuf)
                            .await {
                            Ok(dir) => {
                                println!("'{}' created.", dir.uuid())
                            },
                            Err(e) => {
                                println!("{e}")
                            }
                        }
                    },
                    Err(e) => {
                        println!("Invalid path ({e})." )
                    }
                }
            }
            Subcommands::Remove{uuid} => {

            }
            Subcommands::Join{link, path} => {

            }
            Subcommands::Reset => {

            }
        }
    }
    
    async fn number_choice<T: Display>(vec: Vec<T>, head_message: Option<&str>, ) -> usize {
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
}
