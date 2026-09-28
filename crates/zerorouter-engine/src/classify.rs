//! Failure classification, ported from 9router `checkFallbackError` (research R6).

use crate::records::ErrorClass;

/// The class of an upstream HTTP error status.
pub fn by_status(status: u16) -> ErrorClass {
    match status {
        401 | 403 => ErrorClass::Auth,
        404 => ErrorClass::NotFound,
        408 => ErrorClass::Timeout,
        429 => ErrorClass::RateLimited,
        500..=599 => ErrorClass::Transient,
        _ => ErrorClass::RequestError,
    }
}
