//! Shared optimistic version contract for independent private draft slots.
use crate::{Error, Result};

pub(crate) fn next_version(current: u64) -> Result<u64> {
    current
        .checked_add(1)
        .filter(|value| *value <= i64::MAX as u64)
        .ok_or_else(|| Error::Invalid("private draft version exhausted".into()))
}

pub(crate) fn require_version(current: u64, expected: u64) -> Result<()> {
    if current != expected {
        return Err(Error::Conflict(
            "private draft changed on another device; keep your text and inspect the saved draft".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_fit_the_database_and_never_wrap() {
        assert_eq!(next_version(0).unwrap(), 1);
        assert_eq!(next_version(i64::MAX as u64 - 1).unwrap(), i64::MAX as u64);
        assert!(next_version(i64::MAX as u64).is_err());
        assert!(next_version(u64::MAX).is_err());
        assert!(require_version(2, 2).is_ok());
        assert!(require_version(2, 1).is_err());
    }
}
