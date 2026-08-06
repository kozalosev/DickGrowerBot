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

/// Reads a domain value from an environment variable, with a fallback and a lower bound written as
/// the plain numbers they are.
///
/// Use it through [`env_value!`], which is what makes those numbers constants — and so checked
/// while the code is compiled rather than when it is started.
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
/// Each of those numbers becomes a `const` item, which is the whole point: a constant is evaluated
/// while the code is compiled, so `literal`'s validator runs then too and a value the type would
/// refuse **fails the build**. Written as an ordinary call it would only panic at startup, because
/// a `const fn` is evaluated early only where the language requires it — in a constant.
///
/// This works for every domain number, floats included. The restriction that keeps `f64` out of
/// const *generics* (it isn't `Eq`, so it can't be structural-match) says nothing about a `const`
/// item of a type that merely holds one.
macro_rules! env_value {
    ($key:literal : $type:ty $(, or = $default:expr)? $(, at_least = $min:expr)?) => {{
        #[allow(unused_mut)]
        let mut value = $crate::config::env::EnvValue::<$type>::of($key);
        $( value = value.or($crate::literal!($type = $default)); )?
        $( value = value.at_least($crate::literal!($type = $min)); )?
        value.read()
    }};
}

pub(crate) use env_value;

/// A [`Duration`] read from an environment variable holding a whole number of time units.
///
/// The unit, the fallback and the lower bound are three independent choices, and spelling every
/// combination of them out gave a function per combination. Here each is a call of its own, so a
/// new combination costs nothing.
///
/// Everything but the unit is [`EnvValue`]'s: the number is read, defaulted and bounded there, in
/// whichever unit the variable is written in, and only the last step turns it into a [`Duration`].
pub(super) struct EnvDuration<'a> {
    units: EnvValue<'a, u64>,
    seconds_per_unit: u64,
}

impl<'a> EnvDuration<'a> {
    /// The variable holds whole seconds.
    pub fn seconds(key: &'a str) -> Self {
        Self::of(key, 1)
    }

    /// The variable holds whole minutes.
    pub fn minutes(key: &'a str) -> Self {
        Self::of(key, 60)
    }

    fn of(key: &'a str, seconds_per_unit: u64) -> Self {
        Self { units: EnvValue::of(key), seconds_per_unit }
    }

    /// What to read when the variable is missing or unparsable. Zero without this call, which every
    /// optional feature takes to mean "off".
    pub fn or(mut self, default: u64) -> Self {
        self.units = self.units.or(default);
        self
    }

    /// Raises anything smaller, in the same unit — for the intervals a zero would turn into a busy
    /// loop. Without this call nothing is raised, which is not the same as a bound of zero.
    pub fn at_least(mut self, min: u64) -> Self {
        self.units = self.units.at_least(min);
        self
    }

    pub fn read(self) -> Duration {
        Duration::from_secs(self.units.read().saturating_mul(self.seconds_per_unit))
    }
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
    use crate::literal;
    use crate::domain::primitives::{AttemptsCount, Limit};

    const UNSET: &str = "DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET";

    #[test]
    fn a_missing_value_falls_back_to_its_type() {
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount);
        assert_eq!(value, AttemptsCount::default());
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": AttemptsCount, or = 3);
        assert_eq!(value, AttemptsCount::new(3));
        let value = env_value!("DICK_GROWER_BOT_TEST_VARIABLE_THAT_IS_NEVER_SET": Limit, or = 10);
        assert_eq!(value, literal!(Limit = 10));
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

    #[test]
    fn a_missing_variable_is_zero_by_default() {
        assert_eq!(EnvDuration::seconds(UNSET).read(), Duration::ZERO);
        assert_eq!(EnvDuration::minutes(UNSET).read(), Duration::ZERO);
    }

    #[test]
    fn the_fallback_is_read_in_the_unit_it_was_given() {
        assert_eq!(EnvDuration::seconds(UNSET).or(90).read(), Duration::from_secs(90));
        assert_eq!(EnvDuration::minutes(UNSET).or(90).read(), Duration::from_secs(90 * 60));
    }

    #[test]
    fn the_lower_bound_lifts_the_fallback_too() {
        assert_eq!(EnvDuration::seconds(UNSET).at_least(1).read(), Duration::from_secs(1));
        assert_eq!(EnvDuration::minutes(UNSET).at_least(2).read(), Duration::from_secs(120));
        // A fallback above the bound is left where it is.
        assert_eq!(EnvDuration::seconds(UNSET).or(30).at_least(1).read(), Duration::from_secs(30));
        // A fallback below the bound is lifted to it.
        assert_eq!(EnvDuration::seconds(UNSET).or(30).at_least(60).read(), Duration::from_secs(60));
    }
}
