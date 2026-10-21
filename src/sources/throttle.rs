//! The throttling tripwire (owner, 2026-10-19): when a
//! source — above all GIANTS ModHub — answers HTTP 403 or
//! 429, it is saying "stop". A pass that keeps going anyway
//! is how a polite tool becomes a banned one, and a pass
//! that half-finishes must never be written over the good
//! data it was meant to refresh.
//!
//! Sources convert a 403/429 response into the typed
//! [`Throttled`] error at the fetch site, so callers never
//! have to guess what an opaque HTTP error meant. The
//! helpers here also recognize a bare reqwest status error
//! carrying 403/429, for any fetch path that hasn't been
//! converted yet.

use std::fmt;

/// A source answered 403 (Forbidden) or 429 (Too Many
/// Requests). Carries who said it and the exact status, so
/// the halt report can name both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throttled {
    /// Human name of the source that throttled ("GIANTS
    /// ModHub", "KingMods", "itch.io").
    pub source: &'static str,
    /// The HTTP status: 403 or 429.
    pub status: u16,
}

impl fmt::Display for Throttled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} answered HTTP {} (throttled) — the pass halts here",
            self.source, self.status
        )
    }
}

impl std::error::Error for Throttled {}

/// The throttle status in an error, if the error (or
/// anything it wraps) is a 403/429 from a source. Checks the
/// typed [`Throttled`] marker first, then any reqwest error
/// still carrying its status.
pub fn throttle_status(err: &anyhow::Error) -> Option<u16> {
    for cause in err.chain() {
        if let Some(t) = cause.downcast_ref::<Throttled>() {
            return Some(t.status);
        }
        if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
            if let Some(status) = re.status() {
                if status == reqwest::StatusCode::FORBIDDEN
                    || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                {
                    return Some(status.as_u16());
                }
            }
        }
    }
    None
}

/// True when an error means "the source throttled us".
pub fn is_throttled(err: &anyhow::Error) -> bool {
    throttle_status(err).is_some()
}

/// Convert a response's status into a [`Throttled`] error
/// when it is 403/429; Ok otherwise. Fetch sites call this
/// before `error_for_status`, so the typed signal survives
/// whatever context the caller wraps around it.
pub fn check_status(source: &'static str, status: reqwest::StatusCode) -> anyhow::Result<()> {
    if status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        Err(Throttled {
            source,
            status: status.as_u16(),
        }
        .into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_marker_is_recognized_through_context() {
        let err: anyhow::Error = Throttled {
            source: "GIANTS ModHub",
            status: 429,
        }
        .into();
        let err = err.context("a pass wrapped this");
        assert_eq!(throttle_status(&err), Some(429));
        assert!(is_throttled(&err));
    }

    #[test]
    fn plain_errors_are_not_throttles() {
        let err = anyhow::anyhow!("some ordinary failure");
        assert_eq!(throttle_status(&err), None);
        assert!(!is_throttled(&err));
    }

    #[test]
    fn check_status_only_trips_on_403_and_429() {
        assert!(check_status("X", reqwest::StatusCode::OK).is_ok());
        assert!(check_status("X", reqwest::StatusCode::NOT_FOUND).is_ok());
        assert!(check_status("X", reqwest::StatusCode::INTERNAL_SERVER_ERROR).is_ok());
        let e = check_status("X", reqwest::StatusCode::FORBIDDEN).unwrap_err();
        assert_eq!(throttle_status(&e), Some(403));
        let e = check_status("X", reqwest::StatusCode::TOO_MANY_REQUESTS).unwrap_err();
        assert_eq!(throttle_status(&e), Some(429));
    }
}
