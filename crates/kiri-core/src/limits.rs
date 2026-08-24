//! Initial configurable limits (specs/IPC.md).
//!
//! Limits are policy defaults, not ABI constants.

use crate::error::Error;

/// Per-WebView limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// Hard ceiling for a single control payload.
    pub max_control_payload_bytes: u32,
    /// In-flight requests per WebView.
    pub max_inflight_requests: u32,
    /// Outstanding bulk bytes per WebView.
    pub max_outstanding_bulk_bytes: u64,
    /// Ceiling for a single bulk object.
    pub max_single_bulk_bytes: u64,
    /// Open resource handles per WebView.
    pub max_open_resources: u32,
    /// Native menu items accepted in one application-menu snapshot.
    pub max_menu_items: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_control_payload_bytes: crate::constants::DEFAULT_MAX_CONTROL_PAYLOAD_BYTES,
            max_inflight_requests: crate::constants::DEFAULT_MAX_INFLIGHT_REQUESTS,
            max_outstanding_bulk_bytes: crate::constants::DEFAULT_MAX_OUTSTANDING_BULK_BYTES,
            max_single_bulk_bytes: crate::constants::DEFAULT_MAX_SINGLE_BULK_BYTES,
            max_open_resources: crate::constants::DEFAULT_MAX_OPEN_RESOURCES,
            max_menu_items: 64,
        }
    }
}

impl Limits {
    /// Validate a control payload length. `actual` is the observed byte
    /// length of the physical representation.
    pub fn check_control_payload(&self, actual: u32) -> Result<(), Error> {
        if actual > self.max_control_payload_bytes {
            return Err(Error::limit_exceeded(format!(
                "control payload {} bytes exceeds ceiling {}",
                actual, self.max_control_payload_bytes
            )));
        }
        Ok(())
    }

    pub fn check_inflight(&self, current: u32) -> Result<(), Error> {
        if current >= self.max_inflight_requests {
            return Err(Error::busy(format!(
                "in-flight request limit {} reached",
                self.max_inflight_requests
            )));
        }
        Ok(())
    }

    pub fn check_bulk_object(&self, bytes: u64) -> Result<(), Error> {
        if bytes > self.max_single_bulk_bytes {
            return Err(Error::limit_exceeded(format!(
                "bulk object {} bytes exceeds ceiling {}",
                bytes, self.max_single_bulk_bytes
            )));
        }
        Ok(())
    }

    pub fn check_outstanding_bulk(&self, outstanding: u64, additional: u64) -> Result<(), Error> {
        let sum = outstanding.saturating_add(additional);
        if sum > self.max_outstanding_bulk_bytes {
            return Err(Error::limit_exceeded(format!(
                "outstanding bulk bytes {} exceeds ceiling {}",
                sum, self.max_outstanding_bulk_bytes
            )));
        }
        Ok(())
    }

    pub fn check_open_resources(&self, current: u32) -> Result<(), Error> {
        if current >= self.max_open_resources {
            return Err(Error::limit_exceeded(format!(
                "open resource limit {} reached",
                self.max_open_resources
            )));
        }
        Ok(())
    }

    pub fn check_menu_items(&self, current: u32) -> Result<(), Error> {
        if current > self.max_menu_items {
            return Err(Error::limit_exceeded(format!(
                "menu item count {} exceeds ceiling {}",
                current, self.max_menu_items
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn defaults_match_spec() {
        let l = Limits::default();
        assert_eq!(l.max_control_payload_bytes, 1024 * 1024);
        assert_eq!(l.max_inflight_requests, 256);
        assert_eq!(l.max_outstanding_bulk_bytes, 256 * 1024 * 1024);
        assert_eq!(l.max_single_bulk_bytes, 128 * 1024 * 1024);
        assert_eq!(l.max_open_resources, 4096);
    }

    #[test]
    fn oversized_control_payload_rejected() {
        let l = Limits::default();
        let err = l.check_control_payload(l.max_control_payload_bytes + 1).unwrap_err();
        assert_eq!(err.code, ErrorCode::LimitExceeded);
        assert!(l.check_control_payload(l.max_control_payload_bytes).is_ok());
    }

    #[test]
    fn inflight_limit_returns_busy() {
        let l = Limits::default();
        let err = l.check_inflight(l.max_inflight_requests).unwrap_err();
        assert_eq!(err.code, ErrorCode::Busy);
        assert!(l.check_inflight(l.max_inflight_requests - 1).is_ok());
    }

    #[test]
    fn bulk_limits_checked() {
        let l = Limits::default();
        assert_eq!(
            l.check_bulk_object(l.max_single_bulk_bytes + 1).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        assert_eq!(
            l.check_outstanding_bulk(l.max_outstanding_bulk_bytes, 1).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        assert!(l.check_outstanding_bulk(u64::MAX, u64::MAX).is_err(), "overflow must not wrap");
        assert!(l.check_bulk_object(l.max_single_bulk_bytes).is_ok());
    }

    #[test]
    fn resource_limit_checked() {
        let l = Limits::default();
        assert_eq!(
            l.check_open_resources(l.max_open_resources).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
        assert!(l.check_open_resources(0).is_ok());
    }

    #[test]
    fn menu_item_limit_checked() {
        let l = Limits::default();
        assert!(l.check_menu_items(l.max_menu_items).is_ok());
        assert_eq!(
            l.check_menu_items(l.max_menu_items + 1).unwrap_err().code,
            ErrorCode::LimitExceeded
        );
    }
}
