/*
 *            ____           _       __    __     _____ __            ___
 *           / __ \_      __(_)___ _/ /_  / /_   / ___// /___  ______/ (_)___
 *          / / / / | /| / / / __ `/ __ \/ __/   \__ \/ __/ / / / __  / / __ \
 *         / /_/ /| |/ |/ / / /_/ / / / / /_    ___/ / /_/ /_/ / /_/ / / /_/ /
 *        /_____/ |__/|__/_/\__, /_/ /_/\__/   /____/\__/\__,_/\__,_/_/\____/
 *                         /____/
 *     Copyright (C) 2025 Dwight Studio
 *
 *     This program is free software: you can redistribute it and/or modify
 *     it under the terms of the GNU General Public License as published by
 *     the Free Software Foundation, either version 3 of the License, or
 *     (at your option) any later version.
 *
 *     This program is distributed in the hope that it will be useful,
 *     but WITHOUT ANY WARRANTY; without even the implied warranty of
 *     MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *     GNU General Public License for more details.
 *
 *     You should have received a copy of the GNU General Public License
 *     along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */
use std::time::Duration;
use chrono::{DateTime, Utc};
use crate::app::AppMsg;
use crate::{icon_names, util};
use libsyncify::SharedDirectory;
use relm4::adw::prelude::*;
use relm4::factory::FactoryView;
use relm4::prelude::*;
use relm4::{adw, gtk, Sender};
use relm4::gtk::Align;
use tokio::task::JoinHandle;
use tr::tr;
use uuid::Uuid;
use libsyncify::engine::state::Mutation;
use libsyncify::event::SyncifyEvent;

pub struct Overview {
    dir: SharedDirectory,
    tick_handle: JoinHandle<()>,
    sync_state: SyncState,
    last_mutation: Mutation,
}

#[derive(Debug, Clone)]
pub enum OverviewMsg {
    Tick,
    SyncStarted,
    SyncStopped,
    Mutation(Mutation)
}

//noinspection RsSortImplTraitMembers
#[relm4::factory(pub async)]
impl AsyncFactoryComponent for Overview {
    type Init = SharedDirectory;
    type Input = OverviewMsg;
    type Output = AppMsg;
    type CommandOutput = ();
    type ParentWidget = gtk::Box;

    view! {
        adw::PreferencesGroup {
            adw::ActionRow {
                set_title: &self.dir.path().file_name().unwrap().to_string_lossy(),
                set_subtitle: &self.dir.path().to_string_lossy(),
                set_activatable: true,

                add_suffix = &gtk::Image {
                    set_icon_name: Some(icon_names::RIGHT_LARGE)
                },

                connect_activated[sender] => move |_| {
                    sender.output(AppMsg::OpenDetails(uuid)).expect("failed to send output");
                }
            },

            #[name = "sync"]
            adw::ActionRow {
                set_title: &tr!("Loading..."),
                set_subtitle: &tr!("Loading..."),

                #[name = "sync_image"]
                add_prefix = &gtk::Image {
                    set_valign: Align::Center,
                    set_icon_name: Some(icon_names::UPDATE),
                    add_css_class: "status-icon",
                },
            },

            #[name = "mutation"]
            adw::ActionRow {
                set_title: &tr!("Loading..."),
                set_subtitle: &tr!("Loading..."),

                #[name = "mutation_image"]
                add_prefix = &gtk::Image {
                    set_valign: Align::Center,
                    set_icon_name: Some(icon_names::PAPER),
                    add_css_class: "status-icon",
                    add_css_class: "success",
                },
            },

            #[name = "download"]
            adw::ActionRow {
                set_title: &tr!("Loading..."),
                set_subtitle: &tr!("Loading..."),

                #[name = "download_image"]
                add_prefix = &gtk::Image {
                    set_valign: Align::Center,
                    set_icon_name: Some(icon_names::UPDATE),
                    add_css_class: "status-icon",
                },
            },

            #[name = "upload"]
            adw::ActionRow {
                set_title: &tr!("Loading..."),
                set_subtitle: &tr!("Loading..."),

                #[name = "upload_image"]
                add_prefix = &gtk::Image {
                    set_valign: Align::Center,
                    set_icon_name: Some(icon_names::UPDATE),
                    add_css_class: "status-icon",
                },
            }
        }
    }

    async fn init_model(init: Self::Init, index: &DynamicIndex, sender: AsyncFactorySender<Self>) -> Self {

        let message_sender = sender.clone();
        let tick_handle = relm4::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                message_sender.input(OverviewMsg::Tick);
                interval.tick().await;
            }
        });

        let last_mutation = init.current_state().await.head().mutation().clone();

        Self {
            dir: init,
            tick_handle,
            last_mutation,
            sync_state: SyncState::Limited,
        }
    }

    fn init_widgets(
        &mut self,
        _index: &DynamicIndex,
        root: Self::Root,
        _returned_widget: &<Self::ParentWidget as FactoryView>::ReturnedWidget,
        sender: AsyncFactorySender<Self>,
    ) -> Self::Widgets {
        let uuid = self.dir.uuid();
        let widgets = view_output!();

        widgets
    }

    async fn update_with_view(&mut self, widgets: &mut Self::Widgets, message: Self::Input, sender: AsyncFactorySender<Self>) {
        match message {
            OverviewMsg::Tick => {
                // Sync
                match &self.sync_state {
                    SyncState::Limited => {
                        widgets.sync.set_title(&tr!("Local state only"));
                        widgets.sync.set_subtitle(&tr!("No peers are available for synchronization"));
                        widgets.sync_image.set_icon_name(Some(icon_names::CONNECTED_SQUARES_X));
                        widgets.sync_image.set_css_classes(&["status-icon", "warning"]);

                    }
                    SyncState::UpToDate(timestamp) => {
                        widgets.sync.set_title(&tr!("Up to date"));
                        widgets.sync.set_subtitle(&tr!("Last synchronization {} ago", util::human_readable_elapsed(timestamp)));
                        widgets.sync_image.set_icon_name(Some(icon_names::CHECK_ROUND_OUTLINE));
                        widgets.sync_image.set_css_classes(&["status-icon", "success"]);
                    }
                    SyncState::Syncing => {
                        widgets.sync.set_title(&tr!("Synchronization in progress"));
                        widgets.sync.set_subtitle(&tr!("State information are being exchanged with another peer"));
                        widgets.sync_image.set_icon_name(Some(icon_names::UPDATE));
                        widgets.sync_image.set_css_classes(&["status-icon", "accent"]);
                    }
                    SyncState::Conflict => {}
                }

                // Mutation
                match &self.last_mutation {
                    Mutation::Init { timestamp } => {
                        widgets.mutation.set_title(&tr!("Directory initialized"));
                        widgets.mutation.set_subtitle(&tr!("Change saved {} ago", util::human_readable_elapsed(timestamp)));
                    }
                    Mutation::Merge { other_head, timestamp } => {
                        widgets.mutation.set_title(&tr!("Remote state merged"));
                        widgets.mutation.set_subtitle(&tr!("Change saved {} ago", util::human_readable_elapsed(timestamp)));
                    }
                    Mutation::Modify { file_path, timestamp, .. } => {
                        widgets.mutation.set_title(&tr!("{} modified", file_path.split("/").last().unwrap()));
                        widgets.mutation.set_subtitle(&tr!("Change saved {} ago", util::human_readable_elapsed(timestamp)));
                    }
                    Mutation::Move { from, timestamp, .. } => {
                        widgets.mutation.set_title(&tr!("{} moved", from.split("/").last().unwrap()));
                        widgets.mutation.set_subtitle(&tr!("Change saved {} ago", util::human_readable_elapsed(timestamp)));
                    }
                    Mutation::Remove { file_path, timestamp } => {
                        widgets.mutation.set_title(&tr!("{} removed", file_path.split("/").last().unwrap()));
                        widgets.mutation.set_subtitle(&tr!("Change saved {} ago", util::human_readable_elapsed(timestamp)));
                    }
                }
            }
            OverviewMsg::SyncStarted => {
                self.sync_state = SyncState::Syncing;
                sender.input(OverviewMsg::Tick);
            }
            OverviewMsg::SyncStopped => {
                self.sync_state = SyncState::UpToDate(Utc::now());
                sender.input(OverviewMsg::Tick);
            }
            OverviewMsg::Mutation(mutation) => {
                self.last_mutation = mutation;
                sender.input(OverviewMsg::Tick);
            }
        }
    }

    fn shutdown(&mut self, widgets: &mut Self::Widgets, output: Sender<Self::Output>) {
        self.tick_handle.abort();
    }
}

impl Overview {
    pub fn is(&self, uuid: Uuid) -> bool {
        self.dir.uuid() == uuid
    }

    pub fn name(&self) -> String {
        self.dir.name()
    }
}

enum SyncState {
    Limited,
    UpToDate(DateTime<Utc>),
    Syncing,
    Conflict
}