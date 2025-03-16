use crate::parser::utils::read_input;
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use libsyncify::store::link::Link;
use libsyncify::{SharedDirPermission, Syncify};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::exit;
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
        uuid: Option<String>,
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
        let mut syncify = Syncify::new().await.unwrap();

        match &cli.command {
            Subcommands::Sync => {
                if let Err(e) = syncify.start_sync().await {
                    utils::print_error(format!("{e}"));
                    exit(1);
                }
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
                        utils::print_error(
                            "No shared directory found! Please join or create a shared directory!",
                        );
                        exit(1);
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
                    utils::print_error(format!("Invalid UUID: {}", uuid.clone().unwrap()));
                    exit(1);
                }
            }
            Subcommands::List => {
                let dirs = syncify.get_all_shared_directories().await;

                if dirs.is_empty() {
                    utils::print_error("No shared directory found!");
                    exit(1);
                }

                utils::print_clear();
                println!("Shared directory listing:\n");

                for dir in dirs {
                    println!(
                        "'{}' as '{}' in {} mode",
                        dir.path().display(),
                        dir.uuid(),
                        if dir.is_read_only() {
                            "read-only"
                        } else {
                            "write"
                        }
                    )
                }

                utils::print_success("Retrieved all shared directories!");
            }
            Subcommands::Create { path } => match path.parse::<PathBuf>() {
                Ok(pathbuf) => match syncify.create_shared_directory(pathbuf).await {
                    Ok(dir) => {
                        utils::print_success(format!("Shared directory '{}' created.", dir.uuid()));
                    }
                    Err(e) => {
                        utils::print_error(format!("{e}"));
                    }
                },
                Err(e) => utils::print_error(format!("Invalid path ({e}).")),
            },
            Subcommands::Remove { uuid } => {
                if uuid.is_none() {
                    let dirs = syncify.get_all_shared_directories().await;
                    if dirs.is_empty() {
                        utils::print_error("No shared directory to remove!");
                    } else {
                        let dir_choice = utils::number_choice(
                            dirs.iter()
                                .map(|dir| dir.path().canonicalize().unwrap().display().to_string())
                                .collect(),
                            Some("Choose a shared directory to remove:\n(This will not remove any data)"),
                        )
                        .await;

                        Self::remove(syncify, dirs[dir_choice].uuid()).await;
                    }
                } else if let Ok(uuid) = Uuid::parse_str(uuid.clone().unwrap().as_str()) {
                    Self::remove(syncify, uuid).await;
                } else {
                    utils::print_error("Invalid UUID!");
                }
            }
            Subcommands::Join { link, path } => match Link::from_str(link) {
                Ok(o_link) => {
                    todo!()
                }
                Err(e) => {
                    utils::print_error(format!("{e}"));
                }
            },
            Subcommands::Reset => {
                utils::print_clear();
                println!(
                    "ARE YOU SURE YOU WANT TO RESET ? THIS WILL REMOVE ALL YOUR SHARES.\nYour local files will be preserved.\nPlease write {} if you are sure.\n",
                    "YES".bold().underline()
                );
                utils::print_no_newline("Choice> ".purple().bold());
                let resp = read_input();
                if resp.trim_end() == "YES" {
                    for dir in syncify.get_all_shared_directories().await {
                        if let Err(e) = syncify.remove_shared_directory(dir).await {
                            utils::print_error(format!("{e}"));
                        }
                    }
                    utils::print_success("Syncify has been reset!")
                } else {
                    utils::print_success("Your shared directories are left untouched!");
                }
            }
        }
    }

    async fn invite(syncify: Syncify, uuid: Uuid, permission: &InvitePermission) {
        match *permission {
            InvitePermission::ReadOnly => {
                utils::print_success(format!(
                    "Here is the link: {}",
                    Link::builder(syncify)
                        .dir_uuid(uuid)
                        .permission(SharedDirPermission::ReadOnly)
                        .build()
                        .await
                        .unwrap()
                ));
            }
            InvitePermission::Write => {
                if let Ok(link) = Link::builder(syncify).dir_uuid(uuid).build().await {
                    utils::print_success(format!("Here is the link: {link}",));
                } else {
                    utils::print_error(
                        "You cannot share a directory with read-only access as a directory with write permission!",
                    );
                }
            }
        }
    }

    async fn remove(mut syncify: Syncify, uuid: Uuid) {
        if let Some(dir) = syncify.get_shared_directory(&uuid).await {
            let dir_path = dir.path().display().to_string();
            if let Err(e) = syncify.remove_shared_directory(dir).await {
                println!("{}", e);
                exit(1);
            } else {
                utils::print_success(format!(
                    "The shared directory '{}' has been removed!",
                    dir_path
                ))
            }
        } else {
            utils::print_error("The shared directory does not exists!");
        }
    }
}
