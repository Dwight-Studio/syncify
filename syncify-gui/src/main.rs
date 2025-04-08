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
use relm4::adw::gio::{resources_register, Resource};
use relm4::adw::glib;
use crate::app::App;
use crate::error::Error;
use libsyncify::Syncify;
use libsyncify::util::setup_logger;
use relm4::{gtk, RelmApp};
use tr::tr_init;

mod app;
mod error;
mod event_handler;
mod util;
mod widget;

mod rsc {
    include!(concat!(env!("OUT_DIR"), "/resources.rs"));
}

fn main() {
    setup_logger().unwrap();

    // Initialize tr
    tr_init!("/usr/share/locale");

    // Load the ressources
    let bytes = glib::Bytes::from_static(rsc::GRESOURCE_BYTES);
    let resource = Resource::from_data(&bytes).unwrap();
    resources_register(&resource);

    // Initialize GTK
    gtk::init().unwrap();

    // Add the stylesheet and the icons
    let display = gtk::gdk::Display::default().unwrap();
    let provider = gtk::CssProvider::new();
    provider.load_from_resource(&format!{"{}/style.css", rsc::RESOURCE_PREFIX});
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    gtk::IconTheme::for_display(&display).add_resource_path(rsc::RESOURCE_PREFIX);

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
