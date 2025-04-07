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
use chrono::{DateTime, Utc};
use tr::tr;

pub fn human_readable_elapsed(timestamp: &DateTime<Utc>) -> String {
    let delta = Utc::now() - *timestamp;
    
    let weeks = delta.num_weeks();
    
    if weeks != 0 {
        return tr!("a week" | "{} weeks" % weeks);
    }

    let days = delta.num_days();
    
    if days != 0 {
        return tr!("a day" | "{} days" % days);
    }
    
    let hours = delta.num_hours();
    
    if hours != 0 {
        return tr!("an hour" | "{} hours" % hours);
    }
    
    let minutes = delta.num_minutes();
    
    if minutes != 0 {
        return tr!("a minute" | "{} minutes" % minutes);
    }
    
    let seconds = delta.num_seconds();
    
    tr!("a second" | "{} seconds" % seconds)
}