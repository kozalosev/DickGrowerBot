use crate::{f64_domain, number_wrapper};
use derive_more::{Display, Error};

f64_domain!(Ratio);

#[derive(Debug, Display, Error)]
pub struct InvalidRatioValue(#[error(not(source))] f64);

impl Ratio {
    pub fn new(value: f64) -> Result<Self, InvalidRatioValue> {
        if (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidRatioValue(value))
        }
    }
    
    pub fn to_value(self) -> f64 {
        self.0
    }
}
