use num_traits::{Float, FromPrimitive};

/// Convert decibels to linear amplitude (`10^(db/20)`).
pub fn db_to_linear<FloatType>(db: FloatType) -> FloatType
where
    FloatType: Float + FromPrimitive,
{
    let ten = FloatType::from_f64(10.0).unwrap();
    let factor = FloatType::from_f64(0.05).unwrap();
    ten.powf(db * factor)
}
