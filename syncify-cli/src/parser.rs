use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use libsyncify::store::link::Link;
use libsyncify::{SharedDirPermission, Syncify};
use std::fmt::Display;
use std::io::Write;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::sync::mpsc;
use uuid::Uuid;

mod utils;

#[derive(Parser)]
#[command(name = "Syncify Command Line Interface")]
#[command(version, about, long_about = None)]
pub(crate) struct Commands {
    #[command(subcommand)]
    command: Subcommands,

    #[arg(short, long)]
    #[clap(global = true)]
    /// Show the log output
    verbose: bool,
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
        if cli.verbose {
            utils::setup_logger().unwrap();
        }
        let mut cmd = Commands::command();
        let mut syncify = Syncify::new().await.unwrap();

        match &cli.command {
            Subcommands::Sync => {
                syncify.start_sync().await.unwrap();
                //syncify.create_shared_directory(PathBuf::from("target/debug/examples")).await.unwrap();
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

                    if dirs.is_empty() {
                        cmd.error(
                            ErrorKind::Io,
                            "No shared folder found! Please join or create a shared folder!",
                        );
                    }

                    let dir_choice = utils::number_choice(
                        dirs.iter()
                            .map(|dir| dir.path().canonicalize().unwrap().display().to_string())
                            .collect(),
                        Some("Choose a directory to share:"),
                    )
                    .await;

                    let uuid = dirs[dir_choice].uuid();
                    let permission = {
                        if dirs[dir_choice].is_read_only() {
                            InvitePermission::ReadOnly
                        } else {
                            let perm_vec: HashMap<&str, InvitePermission> = vec![
                                ("Write", InvitePermission::Write),
                                ("Read-Only", InvitePermission::ReadOnly),
                            ]
                            .into_iter()
                            .collect();
                            utils::hash_choice(
                                perm_vec,
                                Some("Choose the directory access policy:"),
                            )
                            .await
                        }
                    };
                    Self::invite(syncify, uuid, &permission).await;
                } else if let Ok(uuid) = Uuid::parse_str(uuid.clone().unwrap().as_str()) {
                    Self::invite(syncify, uuid, permission).await;
                } else {
                    cmd.error(
                        ErrorKind::InvalidValue,
                        format!("Invalid UUID: {}", uuid.clone().unwrap()),
                    );
                }
            }
            Subcommands::List => {
                for dir in syncify.get_all_shared_directories().await {
                    println!(
                        "'{}' as '{}'",
                        dir.path().canonicalize().unwrap().display(),
                        dir.uuid()
                    )
                }
            }
            Subcommands::Create { path } => match path.parse::<PathBuf>() {
                Ok(pathbuf) => match syncify.create_shared_directory(pathbuf).await {
                    Ok(dir) => {
                        println!("'{}' created.", dir.uuid())
                    }
                    Err(e) => {
                        println!("{e}")
                    }
                },
                Err(e) => {
                    println!("Invalid path ({e}).")
                }
            },
            Subcommands::Remove { uuid } => {
                if let Ok(uuid) = Uuid::parse_str(uuid.clone().as_str()) {
                    if let Some(dir) = syncify.get_shared_directory(&uuid).await {
                        if let Err(e) = syncify.remove_shared_directory(dir).await {
                            println!("{}", e)
                        }
                    } else {
                        cmd.error(ErrorKind::Io, "The shared directory does not exists!");
                    }
                }
            }
            Subcommands::Join { link, path } => match Link::from_str(link) {
                Ok(o_link) => {
                    todo!()
                }
                Err(e) => {
                    println!("{}", e)
                }
            },
            Subcommands::Reset => {
                for dir in syncify.get_all_shared_directories().await {
                    if let Err(e) = syncify.remove_shared_directory(dir).await {
                        println!("{}", e)
                    }
                }
            }
        }
    }

    async fn invite(syncify: Syncify, uuid: Uuid, permission: &InvitePermission) {
        let mut cmd = Commands::command();
        match *permission {
            InvitePermission::ReadOnly => {
                println!(
                    "Link: {}",
                    Link::builder(syncify)
                        .dir_uuid(uuid)
                        .permission(SharedDirPermission::ReadOnly)
                        .build()
                        .await
                        .unwrap()
                );
            }
            InvitePermission::Write => {
                if let Ok(link) = Link::builder(syncify).dir_uuid(uuid).build().await {
                    println!("Link: {link}",)
                } else {
                    cmd.error(ErrorKind::Io, "You cannot share a directory with read-only access as a directory with write permission!");
                }
            }
        }
    }
}
