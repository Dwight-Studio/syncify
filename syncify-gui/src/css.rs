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

pub fn get_css() -> String {
    r#"
    .status-icon {
        padding: 10px;
        border-radius: 50%;
    }
    
    .status-icon.success {
        background-color: alpha(var(--success-bg-color), 0.3);
    }
    
    .status-icon.warning {
        background-color: alpha(var(--warning-bg-color), 0.3);
    }
    
    .status-icon.accent {
        background-color: alpha(var(--accent-bg-color), 0.3);
    }
    
    .status-icon.destructive {
        background-color: alpha(var(--destructive-bg-color), 0.3);
    }"#
    .to_string()
}
