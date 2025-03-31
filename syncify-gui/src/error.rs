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
use relm4::{adw, gtk, ComponentParts, ComponentSender, SimpleComponent};
use relm4::adw::prelude::*;
use relm4::gtk::Align;
use libsyncify::SyncifyError;
use crate::icon_names;

pub struct Error {
    error: SyncifyError
}

#[derive(Debug)]
pub enum ErrorMsg {
    Close
}

//noinspection RsSortImplTraitMembers
#[relm4::component(pub)]
impl SimpleComponent for Error {
    type Init = SyncifyError;
    type Input = ErrorMsg;
    type Output = ();

    view! {
        #[name = "main_window"]
        adw::ApplicationWindow {
            set_title: Some("Syncify error"),
            set_resizable: false,

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    set_show_title: true,
                },
                adw::StatusPage {
                    set_title: "Error",
                    set_description: Some("Syncify encountered an error during launch:"),
                    set_icon_name: Some(icon_names::SENTIMENT_DISSATISFIED),

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 20,

                        adw::PreferencesGroup {
                            adw::ActionRow {
                                set_use_markup: false,
                                set_title: &model.error.to_string(),
                                set_title_selectable: true,
                                add_css_class: "monospace"
                            }
                        },

                        gtk::Button {
                            set_label: "Close",
                            add_css_class: "destructive-action",
                            add_css_class: "pill",
                            set_halign: Align::Center,

                            connect_clicked => ErrorMsg::Close
                        }
                    }
                }
            }
        }
    }

    fn init(init: Self::Init, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let model = Error { error: init };

        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, sender: ComponentSender<Self>) {
        relm4::main_application().quit();
    }
}