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
use crate::app::App;
use crate::error::Error;
use libsyncify::Syncify;
use libsyncify::util::setup_logger;
use relm4::RelmApp;
use tr::tr_init;

mod app;
mod css;
mod error;
mod event_handler;
mod util;
mod widget;

mod icon_names {
    include!(concat!(env!("OUT_DIR"), "/icon_names.rs"));
}

fn main() {
    setup_logger().unwrap();

    // Initialize tr
    tr_init!("/usr/share/locale");

    relm4_icons::initialize_icons(icon_names::GRESOURCE_BYTES, icon_names::RESOURCE_PREFIX);
    
    relm4::set_global_css(&css::get_css());

    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(Syncify::new());

    match result {
        Ok(syncify) => {
            let app = RelmApp::new("fr.dwightstudio.syncify");
            app.run_async::<App>(syncify)
        }

        Err(e) => {
            let app = RelmApp::new("fr.dwightstudio.syncify");
            app.run::<Error>(e)
        }
    }
}
