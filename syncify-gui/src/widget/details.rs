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
use relm4::adw::prelude::NavigationPageExt;
use relm4::prelude::*;
use tr::tr;
use uuid::Uuid;
use libsyncify::SharedDirectory;

pub struct Details {
    dir: SharedDirectory
}

#[derive(Debug)]
pub enum DetailsMsg {}

#[derive(Debug)]
pub enum DetailsOutput {
    Remove(Uuid),
}

//noinspection RsSortImplTraitMembers
#[relm4::component(pub async)]
impl AsyncComponent for Details {
    type Init = SharedDirectory;
    type Input = DetailsMsg;
    type Output = DetailsOutput;
    type CommandOutput = ();

    view! {
        #[name("page")]
        adw::NavigationPage {
            set_title: &tr!("{} - Details", model.dir.path().file_name().unwrap().to_string_lossy()),
        }
    }

    async fn init(init: Self::Init, root: Self::Root, sender: AsyncComponentSender<Self>) -> AsyncComponentParts<Self> {
        let model = Self {
            dir: init
        };

        let widgets = view_output!();

        AsyncComponentParts { model, widgets }
    }
}