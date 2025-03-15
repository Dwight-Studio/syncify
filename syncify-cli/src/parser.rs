use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use libsyncify::{SharedFolderPermission, Syncify};
use std::path::PathBuf;
use tokio::sync::mpsc;
use uuid::Uuid;

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
        #[arg(value_enum)]
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
                    todo!();
                } else if let Ok(uuid_str) = Uuid::from_slice(uuid.clone().unwrap().as_bytes()) {
                    match *permission {
                        InvitePermission::ReadOnly => {
                            println!(
                                "{}",
                                syncify
                                    .build_link(uuid_str, SharedFolderPermission::ReadOnly)
                                    .await
                                    .unwrap()
                            );
                        }
                        InvitePermission::Write => {
                            println!(
                                "{}",
                                syncify
                                    .build_link(uuid_str, SharedFolderPermission::Write)
                                    .await
                                    .unwrap()
                            )
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
            _ => {
                todo!();
            }
        }
    }
}
