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
use crate::icon_names;
use crate::widget::details::Details;
use crate::widget::overview::Overview;
use libsyncify::Syncify;
use log::warn;
use relm4::adw::glib;
use relm4::adw::prelude::*;
use relm4::loading_widgets::LoadingWidgets;
use relm4::prelude::*;
use relm4::{AsyncComponentSender, adw, gtk, view};
use std::collections::HashMap;
use uuid::Uuid;

pub struct App {
    syncify: Syncify,
    overview_dirs: AsyncFactoryVecDeque<Overview>,
    details_dirs: HashMap<Uuid, AsyncController<Details>>,
}

#[derive(Debug)]
pub enum AppMsg {
    Create,
    Open(Uuid),
    Add(Uuid),
    Remove(Uuid),
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
        adw::ApplicationWindow {
            set_title: Some("Syncify"),
            set_default_width: 960,
            set_default_height: 540,
            //set_hide_on_close: true,

            adw::ToastOverlay {
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
                                    set_icon_name: icon_names::PLUS_LARGE,
                                },

                                pack_end = &gtk::Button {
                                    set_icon_name: icon_names::MENU_LARGE,
                                }
                            },

                            adw::Clamp {
                                set_maximum_size: 600,
                                set_tightening_threshold: 400,

                                adw::StatusPage {

                                    #[local_ref]
                                    dirs_box -> gtk::Box {
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

    async fn init(init: Self::Init, root: Self::Root, sender: AsyncComponentSender<Self>) -> AsyncComponentParts<Self> {
        // Overview
        let mut overview_dirs = AsyncFactoryVecDeque::builder()
            .launch_default()
            .forward(sender.input_sender(), std::convert::identity);

        // Details
        let mut details_dirs = HashMap::new();

        {
            let mut guard = overview_dirs.guard();

            for dir in init.get_all_shared_directories().await {
                guard.push_back(dir.clone());
                details_dirs.insert(
                    dir.uuid(),
                    Details::builder()
                        .launch(dir.clone())
                        .forward(sender.input_sender(), std::convert::identity),
                );
            }
        }

        let model = Self {
            syncify: init,
            overview_dirs,
            details_dirs,
        };

        let dirs_box = model.overview_dirs.widget();
        let widgets = view_output!();

        // Property bindings
        widgets
            .main_window
            .bind_property("title", &widgets.main_page, "title")
            .flags(glib::BindingFlags::SYNC_CREATE)
            .build();

        AsyncComponentParts { model, widgets }
    }

    fn init_loading_widgets(root: Self::Root) -> Option<LoadingWidgets> {
        view! {
            #[local]
            root {
                #[name(spinner)]
                gtk::Spinner {
                    start: (),
                    set_hexpand: true,
                    set_halign: gtk::Align::Center,
                    // Reserve vertical space
                    //set_height_request: 34,
                }
            }
        }
        Some(LoadingWidgets::new(root, spinner))
    }

    async fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        message: Self::Input,
        sender: AsyncComponentSender<Self>,
        _root: &Self::Root,
    ) {
        let mut od_guard = self.overview_dirs.guard();
        match message {
            AppMsg::Create => {}

            AppMsg::Open(uuid) => {
                if let Some(dir) = self.syncify.get_shared_directory(&uuid).await {
                    let controller = Details::builder().launch(dir.clone()).forward(sender.input_sender(), std::convert::identity);
                    widgets.nav_view.push(controller.widget())
                }
            }

            AppMsg::Add(uuid) => {
                if let Some(dir) = self.syncify.get_shared_directory(&uuid).await {
                    od_guard.push_back(dir);
                } else {
                    warn!("Cannot add directory: Not found");
                }
            }

            AppMsg::Remove(uuid) => {
                // Remove overview
                for i in 0..od_guard.len() {
                    if let Some(dir) = od_guard.get(i) {
                        if dir.is(uuid) {
                            od_guard.remove(i);
                            break;
                        }
                    }
                }
            }
        }
    }
}
