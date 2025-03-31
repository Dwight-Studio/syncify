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
use libsyncify::SharedDirectory;
use relm4::adw::prelude::*;
use relm4::prelude::*;
use relm4::{adw, gtk};
use relm4::factory::FactoryView;
use tr::tr;
use uuid::Uuid;
use crate::app::AppMsg;

pub struct Overview {
    dir: SharedDirectory
}

#[derive(Debug)]
pub enum OverviewMsg {}

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
                    sender.output(AppMsg::Open(uuid)).expect("failed to send output");
                }
            },

            adw::ActionRow {
                set_title: &tr!("Up to date"),
                set_subtitle: &tr!("Last update 5 minutes ago"),

                add_prefix = &gtk::Image {
                    set_icon_name: Some(icon_names::CHECK_ROUND_OUTLINE),
                    add_css_class: "success",
                },
            }
        }
    }

    async fn init_model(init: Self::Init, index: &DynamicIndex, sender: AsyncFactorySender<Self>) -> Self {
        Self { dir: init }
    }

    fn init_widgets(&mut self, index: &DynamicIndex, root: Self::Root, returned_widget: &<Self::ParentWidget as FactoryView>::ReturnedWidget, sender: AsyncFactorySender<Self>) -> Self::Widgets {
        let uuid = self.dir.uuid();
        let widgets = view_output!();
        
        widgets
    }
}

impl Overview {
    pub fn is(&self, uuid: Uuid) -> bool {
        self.dir.uuid() == uuid
    }
}