use crate::SharedDirectory;
use iroh_gossip::net::GossipSender;
use log::info;
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::EventKind::{Create, Modify, Remove};

pub fn handle_events(dir: &SharedDirectory, topic: &GossipSender, fs_event: notify::Event) {
    info!("Dir {}: {:?}", dir.uuid(), fs_event.kind);
    match fs_event.kind {
        Create(kind) => match kind {
            CreateKind::File => {}
            _ => {}
        },
        Modify(kind) => match kind {
            ModifyKind::Data(_) => {}
            ModifyKind::Name(_) => {}
            _ => {}
        },
        Remove(kind) => match kind {
            RemoveKind::File => {}
            _ => {}
        },
        _ => (),
    }
}