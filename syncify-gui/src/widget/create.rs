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
use std::path::PathBuf;
use std::str::FromStr;
use relm4::adw::prelude::*;
use relm4::gtk::Align;
use relm4::prelude::*;
use tr::tr;
use libsyncify::{SharedDirectory, Syncify, SyncifyError};
use libsyncify::store::link::Link;
use crate::app::AppMsg;
use crate::icon_names;

pub struct CreateDialog {
    syncify: Syncify,
    path: Option<PathBuf>,
}

#[derive(Debug)]
pub enum CreateDialogMsg {
    OpenFileDialog,
    Toggle,
    SelectJoin,
    Create,
}

//noinspection RsSortImplTraitMembers
#[relm4::component(pub async)]
impl AsyncComponent for CreateDialog {
    type Init = Syncify;
    type Input = CreateDialogMsg;
    type Output = AppMsg;
    type CommandOutput = ();

    view! {
        adw::Dialog::builder()
            .title(&tr!("Add a Shared Directory"))
            .content_width(400)
            .content_height(400)
            .build()
        {
            #[wrap(Some)]
            set_child = &adw::ToolbarView {
                add_top_bar = &adw::HeaderBar,

                adw::Clamp{
                    set_maximum_size: 400,
                    set_tightening_threshold: 300,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 20,

                        adw::PreferencesGroup {

                            #[name = "dir_row"]
                            adw::ActionRow {
                                set_title: &tr!("Directory"),
                                set_activatable: true,

                                add_suffix = &gtk::Button {
                                    set_valign: Align::Center,

                                    connect_clicked => CreateDialogMsg::OpenFileDialog,

                                    gtk::Box {
                                        set_spacing: 6,

                                        gtk::Image {
                                            set_icon_name: Some(icon_names::FOLDER_OPEN)
                                        },

                                        #[name = "open_label"]
                                        gtk::Label {
                                            set_text: &tr!("Open..."),
                                        }
                                    }
                                }
                            },
                        },

                        adw::PreferencesGroup {
                            set_title: &tr!("Identity"),

                            adw::ActionRow {
                                set_title: &tr!("Create a new Shared Directory"),
                                set_activatable: true,

                                #[name = "radio_1"]
                                add_prefix = &gtk::CheckButton {
                                    set_active: true,

                                    connect_toggled => CreateDialogMsg::Toggle
                                },

                                set_activatable_widget: Some(&radio_1)
                            },

                            #[name = "expander"]
                            adw::ExpanderRow {
                                set_title: &tr!("Join an existing Shared Directory"),
                                set_activatable: true,

                                #[name = "radio_2"]
                                add_prefix = &gtk::CheckButton {
                                    set_active: false,
                                    set_group: Some(&radio_1),
                                },

                                connect_expanded_notify => CreateDialogMsg::SelectJoin,

                                #[name = "link"]
                                add_row = &adw::EntryRow {
                                    set_title: &tr!("Access link"),
                                }
                            },
                        },

                        #[name = "btn"]
                        gtk::Button {
                            set_label: &tr!("Create"),
                            add_css_class: "suggested-action",
                            add_css_class: "pill",
                            set_halign: Align::Center,
                            set_sensitive: false,

                            connect_clicked => CreateDialogMsg::Create
                        }
                    }
                }
            }
        }
    }

    async fn init(init: Self::Init, root: Self::Root, sender: AsyncComponentSender<Self>) -> AsyncComponentParts<Self> {
        let model = CreateDialog { syncify: init, path: None };
        let widgets = view_output!();

        AsyncComponentParts { model, widgets }
    }

    async fn update_with_view(&mut self, widgets: &mut Self::Widgets, message: Self::Input, sender: AsyncComponentSender<Self>, root: &Self::Root) {
        match message {
            CreateDialogMsg::OpenFileDialog => {
                let file_dialog = gtk::FileDialog::builder()
                    .title(tr!("Select a Directory"))
                    .accept_label(tr!("Open"))
                    .build();

                if let Ok(dir) = file_dialog.select_folder_future(root.toplevel_window().as_ref()).await {
                    self.path = dir.path();
                    widgets.dir_row.remove_css_class("error");
                    widgets.btn.set_sensitive(true);
                }

                if let Some(path) = &self.path {
                    widgets.open_label.set_text(&path.file_name().unwrap().to_string_lossy())
                }
            }

            CreateDialogMsg::Toggle => {
                if widgets.radio_1.is_active() {
                    widgets.expander.set_expanded(false);
                }
            }


            CreateDialogMsg::SelectJoin => {
                widgets.radio_2.set_active(true)
            }

            CreateDialogMsg::Create => {
                if let Some(path) = &self.path {
                    if path.is_dir() && path.metadata().map(|m| !m.permissions().readonly()).is_ok_and(|b| b) {
                        if widgets.radio_1.is_active() {
                            match self.syncify.create_shared_directory(path.clone()).await {
                                Ok(dir) => {
                                    self.close(dir, widgets, sender);
                                }
                                Err(e) => {
                                    self.error(e, root.widget_ref());
                                }
                            }
                        } else if let Ok(link) = Link::from_str(widgets.link.text().as_str()) {
                            match self.syncify.join_shared_directory(link, path.clone()).await {
                                Ok(dir) => {
                                    self.close(dir, widgets, sender);
                                }
                                Err(e) => {
                                    self.error(e, root.widget_ref());
                                }
                            }
                        } else {
                            widgets.expander.add_css_class("error");
                        }
                    } else {
                        widgets.dir_row.add_css_class("error")
                    }
                } else {
                    widgets.dir_row.add_css_class("error")
                }
            }
        }
    }
}

impl CreateDialog {
    fn close(&mut self, dir: SharedDirectory, widgets: &mut <CreateDialog as AsyncComponent>::Widgets, sender: AsyncComponentSender<Self>) {
        sender.output(AppMsg::Add(dir.uuid())).expect("failed to send output");
        widgets.btn.set_sensitive(false);
        widgets.open_label.set_text(&tr!("Open"));
        widgets.dir_row.remove_css_class("error");
        widgets.expander.remove_css_class("error");
        self.path = None;
    }
    
    fn error(&self, e: SyncifyError, root: &gtk::Widget) {
        let dialog = adw::AlertDialog::builder()
            .heading(tr!("Error"))
            .body(e.to_string())
            .body_use_markup(false)
            .build();

        dialog.add_response("close", &tr!("Close"));
        dialog.present(Some(root));
    }
}