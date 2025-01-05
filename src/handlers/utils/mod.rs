pub mod page;
pub mod callbacks;
pub mod locks;
mod tghack;
mod incrementor;

pub use tghack::*;
pub use incrementor::*;

use teloxide::types::User;
use crate::domain::Username;

pub fn get_full_name(user: &User) -> Username {
    let name = user.last_name.as_ref()
        .map(|last_name| format!("{} {}", user.first_name, last_name))
        .unwrap_or(user.first_name.clone());
    Username::new(name)
}

pub mod date {
    use std::borrow::Cow;
    use chrono::{DateTime, Duration, Timelike, Utc};
    use rust_i18n::t;

    pub fn get_time_till_next_day_string(lang_code: &str) -> Cow<str> {
        let now = if cfg!(test) {
            DateTime::parse_from_rfc3339("2023-10-21T22:10:57+00:00")
                .expect("invalid datetime string")
                .into()
        } else {
            Utc::now()
        };
        Some(now + Duration::days(1))
            .and_then(|d| d.with_hour(0))
            .and_then(|d| d.with_minute(0))
            .and_then(|d| d.with_second(0))
            .map(|tomorrow| tomorrow - now)
            .map(|time_left| {
                let hrs = time_left.num_hours();
                let mins = time_left.num_minutes() - hrs * 60;
                t!("titles.time_till_next_day.some", locale = lang_code,
                    hours = hrs, minutes = mins)
            })
            .unwrap_or(t!("titles.time_till_next_day.none", locale = lang_code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_time_till_next_day_string() {
        let expected = "<b>1</b>h <b>49</b>m.";
        let actual = date::get_time_till_next_day_string("en");
        let actual = &actual[actual.len()-expected.len()..];
        assert_eq!(expected, actual)
    }
}
