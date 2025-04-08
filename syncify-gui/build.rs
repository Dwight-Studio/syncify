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
use std::io::Write;
use gvdb::gresource::{BundleBuilder, FileData, PreprocessOptions};
use std::path::{Path, PathBuf};
use std::{env, fs};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::BufWriter;

// Build script for Syncify
// The ressource handling has been largely inspired
// by relm4-icons-build (https://github.com/Relm4/icons)

/// GRessource prefix.
pub const PREFIX: &str = "/fr/dwightstudio/syncify";

/// Ressource directory.
pub const RESOURCES_DIR_NAME: &str = "rsc";

/// Unique identifier added to the icon names.
pub const UNIQUE_IDENTIFIER: &str = "syncify";

fn main() {
    // Resources
    bundle_resources()
}

fn bundle_resources() {
    let out_dir = env::var("OUT_DIR").expect("Missing OUT_DIR env var");
    let out_dir = Path::new(&out_dir);
    let rsc_dir = Path::new(RESOURCES_DIR_NAME);
    
    // The resource bundle file data
    let mut resource_data = Vec::new();
    
    // The rust source file
    let mut source_file = BufWriter::new(
        File::create(out_dir.join("resources.rs"))
            .expect("Failed to create resource source file"),
    );

    // Tell cargo to rerun the build if something changes
    println!("cargo:rerun-if-changed={RESOURCES_DIR_NAME}");

    // Add all icons
    generate_icons(rsc_dir, &mut resource_data, &mut source_file);

    // Add the stylesheet
    generate_stylesheet(rsc_dir, &out_dir, &mut resource_data);

    // Generate ressource bundle
    let data = BundleBuilder::from_file_data(resource_data)
        .build()
        .expect("Failed to build resource bundle");

    // Write ressource bundle file
    fs::write(out_dir.join("resources.gresource"), data)
        .expect("Failed to write resource bundle file");

    write!(
        source_file,
        "/// GResource file contents\n\
        pub const GRESOURCE_BYTES: &[u8] = include_bytes!(\"resources.gresource\");\n\
        /// Resource prefix used in generated `.gresource` file\n\
        pub const RESOURCE_PREFIX: &str = \"{PREFIX}\";"
    ).expect("Failed to write");

    source_file.flush().expect("Failed to flush source file");
}

fn generate_stylesheet(rsc_dir: &Path, out_dir: &Path, file_data: &mut Vec<FileData>) {
    
    // Tell cargo to rerun the build if something changes
    println!("cargo:rerun-if-changed={RESOURCES_DIR_NAME}/style.scss");

    let mut css_file = BufWriter::new(
        File::create(out_dir.join("style.css"))
            .expect("Failed to create stylesheet file"),
    );
    
    match grass::from_path(
        rsc_dir.join("style.scss"),
        &Default::default()
    ) {
        Ok(mut stylesheet) => {
            // Replace
            stylesheet = stylesheet.replace("gtkalpha", "alpha");
            
            writeln!(css_file, "{}", stylesheet).expect("Failed to write css file");
        },
        Err(e) => {
            panic!("Failed to parse style file: {e}")
        }
    };
    
    css_file.flush().expect("Failed to flush css file");

    // Write the resource bundle file data
    file_data.push(FileData::from_file(
        format!("{PREFIX}/style.css"),
        &out_dir.join("style.css"),
        true,
        &PreprocessOptions::xml_stripblanks(),
    ).expect("Failed to create file data"));
}

fn generate_icons(rsc_dir: &Path, file_data: &mut Vec<FileData>, source_file: &mut BufWriter<File>) {

    // Tell cargo to rerun the build if something changes
    println!("cargo:rerun-if-changed={RESOURCES_DIR_NAME}/icons/");
    
    let read_dir = fs::read_dir(rsc_dir.join("icons"))
        .expect("Couldn't open the resource path (relative to the manifest)");
    for entry in read_dir.flatten() {
        if let Some(icon) = path_to_icon_name(&entry.file_name()) {
            let path = entry.path();
            
            // Write the resource bundle file data
            file_data.push(FileData::from_file(
                format!("{PREFIX}/scalable/actions/{UNIQUE_IDENTIFIER}-{icon}-symbolic.svg"),
                &path,
                true,
                &PreprocessOptions::xml_stripblanks(),
            ).expect("Failed to create file data"));
            
            // Add the rust source constants
            let const_name = icon.to_uppercase().replace('-', "_");
            let path = path.display();
            write!(
                source_file,
                "/// Icon name of the icon `{icon}`, found at `{path}`\n\
            pub const {const_name}: &str = \"{UNIQUE_IDENTIFIER}-{icon}\";\n"
            ).expect("Failed to write")
        }
    }
}

pub fn path_to_icon_name(string: &OsStr) -> Option<String> {
    match string.to_str() {
        Some(string) => {
            if string.ends_with(".svg") {
                Some(
                    string
                        .trim_end_matches("-symbolic.svg")
                        .trim_end_matches(".svg")
                        .to_owned(),
                )
            } else {
                println!("Found non-icon file `{string}`, ignoring");
                None
            }
        }
        None => panic!("Failed to convert file name `{string:?}` to string"),
    }
}