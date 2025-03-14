use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap::error::ErrorKind;
use uuid::Uuid;
use libsyncify::{SharedFolderPermission, Syncify};

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
        permission: InvitePermission
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
            }
            Subcommands::Invite { uuid, permission}  => {
                if uuid.is_none() {
                    todo!();
                } else if let Ok(uuid_str) = Uuid::from_slice(uuid.clone().unwrap().as_bytes()) {
                    match *permission {
                        InvitePermission::ReadOnly => { println!("{}", syncify.build_link(uuid_str, SharedFolderPermission::ReadOnly).await.unwrap()); }
                        InvitePermission::Write => { println!("{}", syncify.build_link(uuid_str, SharedFolderPermission::Write).await.unwrap()) }
                    }
                } else {
                    cmd.error(ErrorKind::InvalidValue, format!("Invalid UUID: {}", uuid.clone().unwrap()));
                }
            }
            Subcommands::Reset => {
                todo!();
            }
        }
    }
}