use derive_more::Display;
use std::error::Error;
use std::str::FromStr;

const NANO_PER_GRAM: u64 = 1_000_000_000;
const MAX_FRACTIONAL_DIGITS: usize = 9;

/// An exact GRAM amount in the smallest on-chain unit, nanoGRAM.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TonAmount(u64);

impl TonAmount {
    pub const ZERO: TonAmount = TonAmount(0);

    pub const fn from_nano(nano: u64) -> Self {
        TonAmount(nano)
    }

    pub const fn as_nano(self) -> u64 {
        self.0
    }

    pub fn checked_add(self, rhs: TonAmount) -> Result<TonAmount, TonAmountError> {
        self.0.checked_add(rhs.0).map(TonAmount).ok_or(TonAmountError::Overflow)
    }

    pub fn checked_sub(self, rhs: TonAmount) -> Result<TonAmount, TonAmountError> {
        self.0
            .checked_sub(rhs.0)
            .map(TonAmount)
            .ok_or(TonAmountError::Underflow)
    }

    pub fn require_non_zero(self) -> Result<TonAmount, TonAmountError> {
        if self == TonAmount::ZERO {
            return Err(TonAmountError::ZeroAmount);
        }
        Ok(self)
    }

    /// Returns a canonical decimal GRAM representation without insignificant zeroes.
    pub fn to_grams_string(self) -> String {
        let whole = self.0 / NANO_PER_GRAM;
        let fractional = self.0 % NANO_PER_GRAM;
        if fractional == 0 {
            return whole.to_string();
        }

        let mut fractional = format!("{fractional:09}");
        let trimmed_len = fractional.trim_end_matches('0').len();
        fractional.truncate(trimmed_len);
        format!("{whole}.{fractional}")
    }
}

impl FromStr for TonAmount {
    type Err = TonAmountError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.starts_with(['+', '-']) {
            return Err(TonAmountError::InvalidFormat);
        }

        let (whole, fractional) = match value.split_once('.') {
            Some((whole, fractional)) if !fractional.contains('.') => (whole, Some(fractional)),
            Some(_) => return Err(TonAmountError::InvalidFormat),
            None => (value, None),
        };
        if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(TonAmountError::InvalidFormat);
        }

        let whole = whole.parse::<u64>().map_err(|_| TonAmountError::Overflow)?;
        let whole_nano = whole.checked_mul(NANO_PER_GRAM).ok_or(TonAmountError::Overflow)?;
        let fractional_nano = match fractional {
            None => 0,
            Some(fractional) => {
                if fractional.is_empty() || fractional.len() > MAX_FRACTIONAL_DIGITS {
                    return Err(TonAmountError::InvalidFractionalPrecision);
                }
                if !fractional.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(TonAmountError::InvalidFormat);
                }
                let fractional_len = fractional.len();
                let fractional = fractional.parse::<u64>().map_err(|_| TonAmountError::Overflow)?;
                let scale = 10_u64.pow((MAX_FRACTIONAL_DIGITS - fractional_len) as u32);
                fractional.checked_mul(scale).ok_or(TonAmountError::Overflow)?
            },
        };

        whole_nano
            .checked_add(fractional_nano)
            .map(TonAmount)
            .ok_or(TonAmountError::Overflow)
    }
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum TonAmountError {
    #[display(fmt = "Invalid GRAM amount")]
    InvalidFormat,
    #[display(fmt = "GRAM amount supports at most 9 fractional digits")]
    InvalidFractionalPrecision,
    #[display(fmt = "GRAM amount is outside the supported range")]
    Overflow,
    #[display(fmt = "GRAM amount underflow")]
    Underflow,
    #[display(fmt = "GRAM amount must be greater than zero")]
    ZeroAmount,
}

impl Error for TonAmountError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats_exact_nano_grams() {
        let amount = "1.777".parse::<TonAmount>().unwrap();

        assert_eq!(amount.as_nano(), 1_777_000_000);
        assert_eq!(amount.to_grams_string(), "1.777");
        assert_eq!("0.010".parse::<TonAmount>().unwrap().as_nano(), 10_000_000);
        assert_eq!("0.000000001".parse::<TonAmount>().unwrap().as_nano(), 1);
        assert_eq!(TonAmount::from_nano(100).to_grams_string(), "0.0000001");
    }

    #[test]
    fn rejects_imprecise_or_invalid_amounts() {
        for value in ["", "+1", "-1", ".1", "1.", "1.0000000001", "1e3", "1.2.3"] {
            assert!(value.parse::<TonAmount>().is_err(), "{}", value);
        }
        assert_eq!(TonAmount::ZERO.require_non_zero(), Err(TonAmountError::ZeroAmount));
    }

    #[test]
    fn checks_bounds_without_rounding() {
        let max = "18446744073.709551615".parse::<TonAmount>().unwrap();

        assert_eq!(max.as_nano(), u64::MAX);
        assert_eq!(max.to_grams_string(), "18446744073.709551615");
        assert_eq!(
            "18446744073.709551616".parse::<TonAmount>(),
            Err(TonAmountError::Overflow)
        );
        assert_eq!(
            TonAmount::ZERO.checked_sub(TonAmount::from_nano(1)),
            Err(TonAmountError::Underflow)
        );
        assert_eq!(max.checked_add(TonAmount::from_nano(1)), Err(TonAmountError::Overflow));
    }
}
