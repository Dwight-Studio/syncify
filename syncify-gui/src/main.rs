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
use log::error;
use libsyncify::{Syncify, SyncifyError};
use libsyncify::util::setup_logger;
use relm4::{RelmApp};
use tr::tr_init;
use crate::error::Error;
use crate::app::App;

mod app;
mod error;
mod widget;

mod icon_names {
    include!(concat!(env!("OUT_DIR"), "/icon_names.rs"));
}


fn main() {
    setup_logger().unwrap();

    // Initialize
    let result = Syncify::new();
    relm4_icons::initialize_icons(icon_names::GRESOURCE_BYTES, icon_names::RESOURCE_PREFIX);

    // Initialize tr
    tr_init!("/usr/share/locale");

    match result {
        Ok(mut syncify) => {
            match tokio::runtime::Runtime::new().unwrap().block_on(syncify.start_sync()) {
                Ok(()) => {
                    let app = RelmApp::new("fr.dwightstudio.syncify");
                    app.run_async::<App>(syncify)
                }
                
                Err(e) => {
                    error(e);
                }
            }
            
        }
        
        Err(e) => {
            error(e);
        }
    }
}

fn error(e: SyncifyError) {
    let app = RelmApp::new("fr.dwightstudio.syncify");
    app.run::<Error>(e)
}
