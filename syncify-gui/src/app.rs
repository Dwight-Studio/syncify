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
use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;
use crate::rsc;
use crate::widget::create::CreateDialog;
use crate::widget::details::Details;
use crate::widget::overview::{Overview, OverviewMsg};
use libsyncify::{SharedDirectory, Syncify};
use log::{debug, warn};
use relm4::adw::Toast;
use relm4::adw::prelude::*;
use relm4::loading_widgets::LoadingWidgets;
use relm4::prelude::*;
use relm4::{AsyncComponentSender, adw, gtk, view, Sender};
use tr::tr;
use uuid::Uuid;
use libsyncify::event::{DirectoryEvent, DownloadEvent, SyncEvent, SyncifyEvent};

pub struct App {
    syncify: Syncify,
    overview_controllers: HashMap<Uuid, AsyncController<Overview>>,
    create_dialog: Option<AsyncController<CreateDialog>>,
    current_details: Option<Uuid>,
    details: Option<AsyncController<Details>>,
}

#[derive(Debug)]
pub enum AppMsg {
    OpenCreateDialog,
    OpenDetails(Uuid),
    Event(SyncifyEvent),
}

//noinspection RsSortImplTraitMembers
#[relm4::component(pub async)]
impl AsyncComponent for App {
    type Init = Syncify;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        #[name = "main_window"]
        adw::ApplicationWindow::builder()
            .title("Syncify")
            .default_width(960)
            .default_height(540)
            .build()
            //set_hide_on_close: true,
        {

            #[name = "nav_view"]
            adw::NavigationView {

                #[name = "main_page"]
                adw::NavigationPage {
                    set_title: "Main page",
                    set_tag: Some("main"),

                    adw::ToolbarView {
                        add_top_bar = &adw::HeaderBar {
                            set_show_title: true,

                            pack_start = &gtk::Button {
                                set_icon_name: rsc::PLUS_LARGE,

                                connect_clicked => AppMsg::OpenCreateDialog,
                            },

                            pack_end = &gtk::Button {
                                set_icon_name: rsc::MENU_LARGE,
                            }
                        },

                        #[name = "toast"]
                        adw::ToastOverlay {
                            adw::Clamp {
                                set_maximum_size: 600,
                                set_tightening_threshold: 400,

                                adw::StatusPage {

                                    #[name = "overview_box"]
                                    gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        set_spacing: 20,
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn init(mut init: Self::Init, root: Self::Root, sender: AsyncComponentSender<Self>) -> AsyncComponentParts<Self> {
        init.start_sync().await.unwrap();

        let mut model = Self {
            syncify: init,
            overview_controllers: HashMap::new(),
            create_dialog: None,
            current_details: None,
            details: None,
        };

        let mut widgets = view_output!();
        
        if cfg!(debug_assertions) {
            widgets.main_window.add_css_class("devel");
        }
        
        // Populate overviews
        for dir in model.syncify.get_all_shared_directories().await {
            model.add_directory(&sender, &mut widgets, dir);
        }

        // Property bindings
        widgets
            .main_window
            .bind_property("title", &widgets.main_page, "title")
            .flags(adw::glib::BindingFlags::SYNC_CREATE)
            .build();

        // Receive Syncify Events
        let mut event_receiver = model.syncify.subscribe();
        let message_sender = sender.clone();
        relm4::spawn(async move {
           while let Ok(event) = event_receiver.recv().await {
               message_sender.input(AppMsg::Event(event));
           }
        });

        AsyncComponentParts { model, widgets }
    }

    fn init_loading_widgets(root: Self::Root) -> Option<LoadingWidgets> {
        view! {
            #[local]
            root {
                #[name(loading)]
                adw::StatusPage {
                    set_title: &tr!("Loading Syncify..."),

                    #[wrap(Some)]
                    set_paintable = &adw::SpinnerPaintable::new(Some(&loading)),
                }
            }
        }
        Some(LoadingWidgets::new(root, loading))
    }

    async fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        message: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match message {
            AppMsg::OpenCreateDialog => {
                let dialog = CreateDialog::builder()
                    .launch(self.syncify.clone())
                    .forward(sender.input_sender(), std::convert::identity);
                dialog.widget().present(Some(&widgets.main_window));

                self.create_dialog = Some(dialog);
            }

            AppMsg::OpenDetails(uuid) => {
                if let Some(dir) = self.syncify.get_shared_directory(&uuid).await {
                    // Create component
                    let details = Details::builder()
                        .launch(dir.clone())
                        .forward(sender.input_sender(), std::convert::identity);
                    widgets.nav_view.push(details.widget());
                    self.details = Some(details);

                    self.current_details = Some(uuid);
                }
            }

            AppMsg::Event(event) => {
                match event {
                    SyncifyEvent::Engine(_) => {}
                    SyncifyEvent::Directory(uuid, event) => match event {
                        DirectoryEvent::Created | DirectoryEvent::Joined => {
                            if let Some(dir) = self.syncify.get_shared_directory(&uuid).await {
                                let dir_name = dir.name();
                                self.add_directory(&sender, widgets, dir);
                                widgets.toast.add_toast(
                                    Toast::builder()
                                        .title(tr!("Directory '{}' has been added", dir_name))
                                        .timeout(5)
                                        .build(),
                                );
                            } else {
                                warn!("Cannot add directory: Not found");
                            }
                        }
                        DirectoryEvent::Removed(dir) => {
                            // Close details if the uuid is the same
                            if let Some(details) = self.current_details {
                                if details == uuid {
                                    widgets.nav_view.pop_to_tag("main");
                                }
                            }

                            // Remove overview
                            if let Some(controller) = self.overview_controllers.get(&uuid) {
                                widgets.overview_box.remove(controller.widget());
                                self.overview_controllers.remove(&uuid);
                            }
                            widgets.toast.add_toast(
                                Toast::builder()
                                    .title(tr!("Directory '{}' has been removed", dir.name()))
                                    .timeout(5)
                                    .build(),
                            );
                        }
                        DirectoryEvent::Sync(event) => match *event {
                            SyncEvent::IncomingStarted(_) | SyncEvent::OutgoingStarted(_) => {
                                if let Some(controller) = self.overview_controllers.get(&uuid) {
                                    controller.emit(OverviewMsg::SyncStarted);
                                }
                            }
                            SyncEvent::IncomingStopped(_) | SyncEvent::OutgoingStopped(_) => {
                                if let Some(controller) = self.overview_controllers.get(&uuid) {
                                    controller.emit(OverviewMsg::SyncStopped);
                                }
                            }
                            SyncEvent::Conflict(_, _) => {}
                            SyncEvent::Unverified(_) => {}
                            SyncEvent::Local(state) | SyncEvent::Remote(state) => {
                                if let Some(controller) = self.overview_controllers.get(&uuid) {
                                    controller.emit(OverviewMsg::Mutation(state.head().mutation()));
                                }
                            }
                        }
                        DirectoryEvent::Peer(_) => {}
                    }
                    SyncifyEvent::Download(event) => match event {
                        DownloadEvent::LocalProvisionUpdate(_) => {}
                        DownloadEvent::RemoteProvisionUpdate(_) => {}
                        DownloadEvent::UploadStarted { .. } => {}
                        DownloadEvent::UploadStopped { .. } => {}
                        DownloadEvent::DownloadStarted { dir_uuid, file_size, .. } => {
                            if let Some(controller) = self.overview_controllers.get(&dir_uuid) {
                                controller.emit(OverviewMsg::StartDownload(file_size));
                            }
                        }
                        DownloadEvent::DownloadProgressed { dir_uuid, .. } => {
                            if let Some(controller) = self.overview_controllers.get(&dir_uuid) {
                                controller.emit(OverviewMsg::ProgressDownload);
                            }
                        }
                        DownloadEvent::DownloadCompleted { .. } => {}
                        DownloadEvent::DownloadCancelled { dir_uuid, remaining, .. } => {
                            if let Some(controller) = self.overview_controllers.get(&dir_uuid) {
                                controller.emit(OverviewMsg::CancelDownload(remaining));
                            }
                        }
                    }
                }
            }
        }
    }

    fn shutdown(&mut self, _widgets: &mut Self::Widgets, _output: Sender<Self::Output>) {
        let handle = relm4::spawn(self.syncify.clone().stop_sync());

        while !handle.is_finished() {
            sleep(Duration::from_millis(100));
        }
    }
}

impl App {
    fn add_directory(&mut self, sender: &AsyncComponentSender<Self>,  widgets: &mut <App as AsyncComponent>::Widgets, dir: SharedDirectory) {
        let uuid = dir.uuid();
        let controller = Overview::builder()
            .launch(dir)
            .forward(sender.input_sender(), std::convert::identity);
        
        let mut after = None;
        
        for (key, val) in &self.overview_controllers {
            if *key > uuid {
                break
            } else {
                after = Some(val.widget())
            }
        }
        debug!("Insert {after:?}");
        controller.widget().insert_after(&widgets.overview_box, after);
        self.overview_controllers.insert(uuid, controller);
    }
}