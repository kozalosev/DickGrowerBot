pub const fn greater_or_equal_to_zero<T>(value: &T) -> bool
where
    T: Copy + PartialOrd<T> + From<i8>,
{
    *value >= T::from(0)
}

pub const fn ratio_range_validator(x: &f64) -> bool {
    *x >= 0.0 && *x <= 1.0
}
