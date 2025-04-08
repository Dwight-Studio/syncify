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
use log::error;
use crate::app::AppMsg;
use crate::{rsc, util};
use libsyncify::SharedDirectory;
use relm4::adw::prelude::*;
use relm4::prelude::*;
use relm4::{adw, gtk, Sender};
use relm4::gtk::Align;
use tokio::task::JoinHandle;
use tr::tr;
use libsyncify::engine::state::Mutation;

pub struct Overview {
    _dir: SharedDirectory,
    tick_handle: JoinHandle<()>,
    sync_state: SyncState,
    last_mutation: Mutation,
    last_download_finished: Option<DateTime<Utc>>,
    chunks_total: u128,
    chunks_done: u128,
}

#[derive(Debug, Clone)]
pub enum OverviewMsg {
    Tick,
    SyncStarted,
    SyncStopped,
    Mutation(Mutation),
    StartDownload(u64),
    ProgressDownload,
    CancelDownload(u64),
}

//noinspection RsSortImplTraitMembers
#[relm4::component(pub async)]
impl AsyncComponent for Overview {
    type Init = SharedDirectory;
    type Input = OverviewMsg;
    type Output = AppMsg;
    type CommandOutput = ();

    view! {
        adw::PreferencesGroup {
            adw::ActionRow {
                set_title: &init.path().file_name().unwrap().to_string_lossy(),
                set_subtitle: &init.path().to_string_lossy(),
                set_activatable: true,

                add_suffix = &gtk::Image {
                    set_icon_name: Some(rsc::RIGHT_LARGE)
                },

                connect_activated[sender] => move |_| {
                    if let Err(e) = sender.output(AppMsg::OpenDetails(uuid)) {
                        error!("Failed to send event ({e:?})");
                    }
                }
            },

            #[name = "sync"]
            adw::ActionRow {
                set_title: &tr!("Loading..."),
                set_subtitle: &tr!("Loading..."),

                #[name = "sync_image"]
                add_prefix = &gtk::Image {
                    set_valign: Align::Center,
                    set_icon_name: Some(rsc::UPDATE),
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
                    set_icon_name: Some(rsc::PAPER),
                    add_css_class: "status-icon",
                    add_css_class: "success",
                },
            },

            #[name = "download"]
            adw::PreferencesRow {
                set_activatable: false,
                add_css_class: "download-row",

                #[wrap(Some)]
                set_child = &gtk::Box {
                    add_css_class: "container",
                    set_orientation: gtk::Orientation::Vertical,
                    set_halign: Align::Fill,

                    gtk::Box {
                        add_css_class: "header",
                        set_spacing: 6,
                        set_orientation: gtk::Orientation::Horizontal,
                        set_valign: Align::Center,

                        #[name = "download_image"]
                        gtk::Image {
                            add_css_class: "status-icon",
                            set_valign: Align::Center,
                            set_icon_name: Some(rsc::UPDATE),
                        },

                        gtk::Box {
                            add_css_class: "titles",
                            set_hexpand: true,
                            set_orientation: gtk::Orientation::Vertical,

                            #[name = "download_title"]
                            gtk::Label {
                                add_css_class: "title",
                                set_hexpand: true,
                                set_halign: Align::Start,
                            },

                            #[name = "download_subtitle"]
                            gtk::Label {
                                add_css_class: "subtitle",
                                set_hexpand: true,
                                set_halign: Align::Start
                            },

                            #[name = "download_progress"]
                            gtk::ProgressBar {
                                set_valign: Align::Center,
                                set_visible: false,
                            },
                        },
                    },
                }
            },
        }
    }

    async fn init(mut init: Self::Init, root: Self::Root, sender: AsyncComponentSender<Self>) -> AsyncComponentParts<Self> {

        let message_sender = sender.clone();
        let tick_handle = relm4::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                message_sender.input(OverviewMsg::Tick);
                interval.tick().await;
            }
        });

        let uuid = init.uuid();
        let widgets = view_output!();

        let last_mutation = init.current_state().await.head().mutation().clone();

        let model = Self {
            _dir: init,
            tick_handle,
            last_mutation,
            sync_state: SyncState::Limited,
            last_download_finished: Some(Utc::now()),
            chunks_total: 0,
            chunks_done: 0,
        };

        AsyncComponentParts { model, widgets }
    }

    async fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        message: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            OverviewMsg::Tick => {
                // Sync
                match &self.sync_state {
                    SyncState::Limited => {
                        widgets.sync.set_title(&tr!("Local state only"));
                        widgets.sync.set_subtitle(&tr!("No peers are available for synchronization"));
                        widgets.sync_image.set_icon_name(Some(rsc::CONNECTED_SQUARES_X));
                        widgets.sync_image.set_css_classes(&["status-icon", "warning"]);

                    }
                    SyncState::UpToDate(timestamp) => {
                        widgets.sync.set_title(&tr!("Up to date"));
                        widgets.sync.set_subtitle(&tr!("Last synchronization {} ago", util::human_readable_elapsed(timestamp)));
                        widgets.sync_image.set_icon_name(Some(rsc::CHECK_ROUND_OUTLINE));
                        widgets.sync_image.set_css_classes(&["status-icon", "success"]);
                    }
                    SyncState::Syncing => {
                        widgets.sync.set_title(&tr!("Synchronization in progress"));
                        widgets.sync.set_subtitle(&tr!("State information are being exchanged with another peer"));
                        widgets.sync_image.set_icon_name(Some(rsc::UPDATE));
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
                    Mutation::Remove { file_path, timestamp, .. } => {
                        widgets.mutation.set_title(&tr!("{} removed", file_path.split("/").last().unwrap()));
                        widgets.mutation.set_subtitle(&tr!("Change saved {} ago", util::human_readable_elapsed(timestamp)));
                    }
                }
                
                // Download
                if let Some(timestamp) = &self.last_download_finished {
                    widgets.download_title.set_text(&tr!("No active download jobs"));
                    widgets.download_subtitle.set_text(&tr!("Last download finished {} ago", util::human_readable_elapsed(timestamp)));
                    widgets.download_image.set_icon_name(Some(rsc::CHECK_ROUND_OUTLINE));
                    widgets.download_image.set_css_classes(&["status-icon", "success"]);
                } else {
                    widgets.download_title.set_text(&tr!("Downloading..."));
                    widgets.download_image.set_icon_name(Some(rsc::FOLDER_DOWNLOAD));
                    widgets.download_image.set_css_classes(&["status-icon", "accent"]);
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
            OverviewMsg::StartDownload(size) => {
                if self.chunks_total == 0 {
                    self.last_download_finished = None;

                    widgets.download_progress.set_visible(true);
                    widgets.download_subtitle.set_visible(false);

                    sender.input(OverviewMsg::Tick);
                }
                self.chunks_total += size as u128;
            }
            OverviewMsg::ProgressDownload => {
                self.chunks_done += 1;
                self.update_progress(widgets, sender);
            }
            OverviewMsg::CancelDownload(remaining) => {
                self.chunks_done += remaining as u128;
                self.update_progress(widgets, sender);
            }
        }
    }

    fn shutdown(&mut self, widgets: &mut Self::Widgets, output: Sender<Self::Output>) {
        self.tick_handle.abort();
    }
}

impl Overview {
    fn update_progress(&mut self, widgets: &mut OverviewWidgets, sender: AsyncComponentSender<Overview>) {
        if self.chunks_total == self.chunks_done {
            self.chunks_done = 0;
            self.chunks_total = 0;
            widgets.download_progress.set_fraction(0f64);
            self.last_download_finished = Some(Utc::now());

            widgets.download_progress.set_visible(false);
            widgets.download_subtitle.set_visible(true);

            sender.input(OverviewMsg::Tick);
        } else {
            widgets.download_progress.set_fraction(self.chunks_done as f64 / self.chunks_total as f64);
        }
    }
}

enum SyncState {
    Limited,
    UpToDate(DateTime<Utc>),
    Syncing,
    Conflict
}