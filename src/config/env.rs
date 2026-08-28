use std::error::Error;
use std::fmt::Display;
use std::str::FromStr;
use std::time::Duration;
use anyhow::anyhow;
use crate::domain::primitives::chat::TelegramChatId;
use crate::domain::primitives::Ratio;

pub(super) fn get_env_mandatory_value<T, E>(key: &str) -> anyhow::Result<T>
where
    T: FromStr<Err = E>,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)?
        .parse()
        .map_err(|e: E| anyhow!(e))
}

pub fn get_env_value_or_default<T, E>(key: &str, default: T) -> T
where
    T: FromStr<Err = E> + Display,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)
        .map_err(|e| {
            tracing::warn!(key = %key, default = %default, "no value was found for an optional environment variable, using the default");
            anyhow!(e)
        })
        .and_then(|v| v.parse()
            .map_err(|e: E| {
                tracing::warn!(key = %key, default = %default, "invalid value of an environment variable, using the default");
                anyhow!(e)
            }))
        .unwrap_or(default)
}

pub(super) fn get_optional_env_value<T>(key: &str) -> T
where
    T: Default + FromStr + Display,
    <T as FromStr>::Err: Error + Send + Sync + 'static
{
    get_env_value_or_default(key, T::default())
}

/// A variable that is set and not empty, which is how an optional integration is switched on. An
/// empty value counts as unset, so a variable left blank in `.env` turns the feature off rather
/// than configuring it with nothing. Nothing is logged: the caller says what the absence means.
pub(super) fn get_optional_env_string(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// Reads a domain value from an environment variable, with a fallback and a lower bound written as
/// the plain numbers they are.
///
/// Use it through [`env_value!`], which wraps those numbers in the domain type.
///
/// The variable itself is parsed by `T`'s own `FromStr`, so a validated type turns a bad value down
/// exactly as it would anywhere else and the fallback takes over.
pub(super) struct EnvValue<'a, T> {
    key: &'a str,
    default: T,
    min: Option<T>,
}

impl <'a, T, E> EnvValue<'a, T>
where
    T: FromStr<Err = E> + Display + PartialOrd + Default,
    E: Error + Send + Sync + 'static
{
    pub fn of(key: &'a str) -> Self {
        Self { key, default: T::default(), min: None }
    }

    /// What to read when the variable is missing or unparsable. `T::default()` without this call.
    pub fn or(mut self, default: T) -> Self {
        self.default = default;
        self
    }

    /// Raises anything smaller. Without this call nothing is raised, which is not the same as a
    /// bound of zero.
    pub fn at_least(mut self, min: T) -> Self {
        self.min = Some(min);
        self
    }

    pub fn read(self) -> T {
        let value = get_env_value_or_default(self.key, self.default);
        match self.min {
            Some(min) if value < min => min,
            _ => value,
        }
    }
}

/// Reads an environment variable into a domain type, taking the fallback and the lower bound as
/// bare numbers.
///
/// Both go through the type's `new`, so a number too large for the inner type fails the build.
/// Every type used here validates nothing, which is why `new` is enough. A validated type's `new`
/// returns a `Result` and would not compile in this position — that is the signal to write the
/// bound as a `literal!(...)` and hand it over already built.
macro_rules! env_value {
    ($key:literal : $type:ty $(, or = $default:expr)? $(, at_least = $min:expr)?) => {{
        #[allow(unused_mut)]
        let mut value = $crate::config::env::EnvValue::<$type>::of($key);
        $( value = value.or(<$type>::new($default)); )?
        $( value = value.at_least(<$type>::new($min)); )?
        value.read()
    }};
}

pub(crate) use env_value;

/// A [`Duration`] read from an environment variable that carries its unit in the value.
///
/// **The value says the unit**, as a letter after the number: `15m`, `1h`, `3d`, `30s`. So a knob
/// has one name whatever anyone writes it in, and changing `900` to `15m` is an edit to the value
/// alone — no rename, no code change, and no way for a name and a number to end up disagreeing.
///
/// A bare number is seconds, which is what every value written before this meant.
pub struct EnvDuration<'a> {
    key: &'a str,
    default: Duration,
    min: Option<Duration>,
}

impl<'a> EnvDuration<'a> {
    pub fn new(key: &'a str) -> Self {
        Self { key, default: Duration::ZERO, min: None }
    }

    /// What to read when the variable is missing or unreadable. Zero without this call, which every
    /// optional feature takes to mean "off".
    pub fn or(mut self, default: Duration) -> Self {
        self.default = default;
        self
    }

    /// Raises anything smaller — for the intervals a zero would turn into a busy loop. Without this
    /// call nothing is raised, which is not the same as a bound of zero.
    pub fn at_least(mut self, min: Duration) -> Self {
        self.min = Some(min);
        self
    }

    pub fn read(self) -> Duration {
        let value = get_optional_env_duration(self.key).unwrap_or(self.default);
        self.min.map_or(value, |min| value.max(min))
    }
}

/// A span written as a number and a unit — `250ms`, `30s`, `15m`, `1h`, `3d` — or a bare number,
/// which is seconds. The letters may be upper case.
pub(super) fn parse_duration(raw: &str) -> Result<Duration, InvalidDuration> {
    let raw = raw.trim();
    let last = raw.chars().next_back().ok_or(InvalidDuration::Empty)?;
    // `ms` is the only unit of two letters, and it ends in the same one as `s`, so it has to be
    // tried first — otherwise every millisecond value would quietly read as that many seconds.
    let (number, millis_per_unit) = if let Some(number) = strip_unit(raw, "ms") {
        (number, 1)
    } else {
        // Only the four ASCII letters are ever sliced off, so the cut can't land inside a character.
        match last.to_ascii_lowercase() {
            's' => (&raw[..raw.len() - 1], 1_000),
            'm' => (&raw[..raw.len() - 1], 60 * 1_000),
            'h' => (&raw[..raw.len() - 1], 60 * 60 * 1_000),
            'd' => (&raw[..raw.len() - 1], 60 * 60 * 24 * 1_000),
            other if other.is_alphabetic() => return Err(InvalidDuration::Unit(last)),
            _ => (raw, 1_000),
        }
    };
    let count: u64 = number.trim().parse().map_err(InvalidDuration::Number)?;
    Ok(Duration::from_millis(count.saturating_mul(millis_per_unit)))
}

/// What comes before `unit`, when the value ends with it. `get` refuses a cut inside a character,
/// so a value ending in a multi-byte character simply doesn't match.
fn strip_unit<'a>(raw: &'a str, unit: &str) -> Option<&'a str> {
    let start = raw.len().checked_sub(unit.len())?;
    raw.get(start..)
        .filter(|tail| tail.eq_ignore_ascii_case(unit))
        .map(|_| &raw[..start])
}

/// Why a value isn't a span. Worth telling apart in the log: a number too large to hold is a
/// different mistake from a letter nobody knows, and both read as "not a duration" otherwise.
#[derive(Debug, derive_more::Display)]
pub(super) enum InvalidDuration {
    #[display("it is empty")]
    Empty,
    #[display("'{_0}' is not a unit — use s, m, h or d")]
    Unit(char),
    #[display("{_0}")]
    Number(std::num::ParseIntError),
}

/// The spans a fallback is written in. `Duration::from_days` is still unstable, so one of the four
/// has to be written out — and once one is, the other three earn their place by keeping every
/// fallback in the config the same shape.
pub const fn secs(count: u64) -> Duration {
    Duration::from_secs(count)
}

pub const fn mins(count: u64) -> Duration {
    Duration::from_mins(count)
}

pub const fn hours(count: u64) -> Duration {
    Duration::from_hours(count)
}

pub const fn days(count: u64) -> Duration {
    Duration::from_hours(count * 24)
}

/// Reads a [`Duration`] from a variable that carries its unit in the value, with a fallback and a
/// lower bound. Shaped like [`env_value!`] above.
///
/// A bound of `secs(1)` is the usual one, and what it guards is an interval of zero, which spins.
macro_rules! env_duration {
    ($key:literal $(, or = $default:expr)? $(, at_least = $min:expr)?) => {{
        #[allow(unused_mut)]
        let mut value = $crate::config::env::EnvDuration::new($key);
        $( value = value.or($default); )?
        $( value = value.at_least($min); )?
        value.read()
    }};
}

pub(crate) use env_duration;

/// The span a variable holds, or nothing when it is unset, empty or unreadable.
///
/// The one place a duration is read. [`EnvDuration`] is this plus a fallback and a floor, and a
/// caller whose default belongs to somebody else — teloxide's own timeouts — takes the `None`
/// instead.
///
/// Both ways of having no value are logged, as [`get_env_value_or_default`] logs its own: what the
/// bot fell back to is worth having in the log of a start, and a variable that was meant to be set
/// and isn't looks the same as one nobody ever set until something says so.
pub(super) fn get_optional_env_duration(key: &str) -> Option<Duration> {
    let raw = get_optional_env_string(key)
        .or_else(|| {
            tracing::warn!(key = %key, "no duration was configured for an optional environment variable");
            None
        })?;
    parse_duration(&raw)
        .inspect_err(|e| tracing::warn!(key = %key, value = raw, error = %e,
            "couldn't read a duration, so it counts as unset"))
        .ok()
}

pub(super) fn get_optional_env_ratio(key: &str) -> Option<Ratio> {
    let value = get_env_value_or_default(key, -1.0);
    Ratio::new(value)
        .inspect_err(|_| tracing::warn!(key = %key, value = %value, "the feature is disabled because of an invalid value"))
        .ok()
}

pub(super) fn get_optional_chat_id(key: &str) -> Option<TelegramChatId> {
    std::env::var(key)
        .ok()
        .filter(|id| !id.is_empty())
        .and_then(|id| id.parse::<i64>()
             .inspect_err(|e| tracing::warn!(key = %key, error = %e, "chat_id is not a number"))
             .ok())
        .map(TelegramChatId::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::primitives::{AttemptsCount, Limit};

    #[test]
    fn a_missing_value_falls_back_to_its_type() {
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount);
        assert_eq!(value, AttemptsCount::default());
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount, or = 3);
        assert_eq!(value, AttemptsCount::new(3));
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": Limit, or = 10);
        assert_eq!(value, Limit::new(10));
    }

    #[test]
    fn the_lower_bound_lifts_the_fallback_of_a_value_too() {
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount, at_least = 1);
        assert_eq!(value, AttemptsCount::new(1));
        // A fallback above the bound is left where it is.
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount, or = 3, at_least = 1);
        assert_eq!(value, AttemptsCount::new(3));
        // A fallback below the bound is lifted to it.
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount, or = 1, at_least = 2);
        assert_eq!(value, AttemptsCount::new(2));
    }

    /// What every duration in the configuration is read by, so a wrong answer here is a value out
    /// by sixty or by eighty-six thousand.
    #[test]
    fn a_value_says_its_own_unit() {
        assert_eq!(parse_duration("250ms").ok(), Some(Duration::from_millis(250)));
        assert_eq!(parse_duration("30s").ok(), Some(secs(30)));
        assert_eq!(parse_duration("15m").ok(), Some(mins(15)));
        assert_eq!(parse_duration("1h").ok(), Some(hours(1)));
        assert_eq!(parse_duration("3d").ok(), Some(days(3)));
        // The letters may be shouted, and the value may have been typed with a space around it.
        assert_eq!(parse_duration("15M").ok(), Some(mins(15)));
        assert_eq!(parse_duration(" 1H ").ok(), Some(hours(1)));
        assert_eq!(parse_duration("250MS").ok(), Some(Duration::from_millis(250)));
    }

    /// `ms` and `m` end in different letters but `ms` and `s` do not, so the only way to read `5ms`
    /// as five seconds is to look at the last letter first. A thousandfold error, and a silent one.
    #[test]
    fn milliseconds_are_not_mistaken_for_seconds_or_minutes() {
        assert_eq!(parse_duration("5ms").ok(), Some(Duration::from_millis(5)));
        assert_ne!(parse_duration("5ms").ok(), Some(secs(5)));
        assert_ne!(parse_duration("5ms").ok(), Some(mins(5)));
        // And the units of one letter still mean what they did.
        assert_eq!(parse_duration("5s").ok(), Some(secs(5)));
        assert_eq!(parse_duration("5m").ok(), Some(mins(5)));
    }

    /// Every value written before units were understood still means what it did, which is what lets
    /// a variable be renamed without its value being touched in the same breath.
    #[test]
    fn a_bare_number_is_seconds() {
        assert_eq!(parse_duration("900").ok(), Some(secs(900)));
        assert_eq!(parse_duration("0").ok(), Some(Duration::ZERO));
    }

    #[test]
    fn anything_else_is_not_a_duration() {
        for raw in ["", "   ", "5x", "m", "ms", "5m30s", "-5", "1.5h", "five", "5м", "5xs"] {
            assert!(parse_duration(raw).is_err(), "{raw:?} must not read as a duration");
        }
    }

    /// The log says which mistake it was, because a number too large to hold and a letter nobody
    /// knows are different things to go and fix.
    #[test]
    fn the_reason_is_worth_reading() {
        let too_large = parse_duration("99999999999999999999999s")
            .expect_err("a number that cannot be held must be refused");
        assert!(too_large.to_string().contains("too large"), "got {too_large}");

        let bad_unit = parse_duration("5x").expect_err("x is not a unit");
        assert_eq!(bad_unit.to_string(), "'x' is not a unit — use s, m, h or d");

        let empty = parse_duration("  ").expect_err("nothing is not a duration");
        assert_eq!(empty.to_string(), "it is empty");
    }

    #[test]
    fn a_missing_variable_is_zero_by_default() {
        assert_eq!(env_duration!("DICK_GROWER_BOT_TEST_DURATION"), Duration::ZERO);
    }

    #[test]
    fn the_fallback_is_used_when_nothing_is_set() {
        assert_eq!(env_duration!("DICK_GROWER_BOT_TEST_DURATION", or = mins(15)), secs(900));
    }

    #[test]
    fn the_lower_bound_lifts_the_fallback_too() {
        // Nothing set and no fallback, so the bound is all that is left — which is the interval of
        // zero it exists to prevent.
        let value = env_duration!("DICK_GROWER_BOT_TEST_DURATION", at_least = secs(1));
        assert_eq!(value, secs(1));
        // A fallback above the bound is left where it is.
        let value = env_duration!("DICK_GROWER_BOT_TEST_DURATION", or = secs(30), at_least = secs(1));
        assert_eq!(value, secs(30));
        // And one below it is lifted.
        let value = env_duration!("DICK_GROWER_BOT_TEST_DURATION", or = secs(30), at_least = mins(1));
        assert_eq!(value, mins(1));
    }
}
