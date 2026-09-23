//! Host-owned menu accelerator parsing shared by both native menu adapters.
//!
//! The string comes only from the host `MenuAllowlist`; frontend payloads can
//! never supply or override it. An invalid string fails closed: the whole menu
//! apply returns an explicit error instead of silently dropping the shortcut.

use std::str::FromStr;

use kiri_core::app_menu::MenuItem;
use kiri_core::error::{Error, Result};
use muda::accelerator::Accelerator;

/// Parse the host-owned accelerator string on an allowlisted item into a muda
/// `Accelerator`. `None` parses to `None`; an invalid string is an
/// `invalid_argument` error that aborts the menu apply.
pub fn parse_accelerator(item: &MenuItem) -> Result<Option<Accelerator>> {
    item.accelerator.as_deref().map(Accelerator::from_str).transpose().map_err(|e| {
        Error::invalid_argument(format!(
            "kiri.menu item '{}' has an invalid host accelerator: {e}",
            item.id
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(accelerator: Option<&str>) -> MenuItem {
        MenuItem {
            id: "quit".into(),
            label: "Quit".into(),
            action: "quit".into(),
            accelerator: accelerator.map(str::to_string),
        }
    }

    #[test]
    fn none_parses_to_none() {
        assert_eq!(parse_accelerator(&item(None)).unwrap(), None);
    }

    #[test]
    fn cmd_or_ctrl_parses() {
        let accel = parse_accelerator(&item(Some("CmdOrCtrl+Q"))).unwrap().unwrap();
        assert_eq!(accel.key(), muda::accelerator::Code::KeyQ);
    }

    #[test]
    fn invalid_accelerator_fails_closed() {
        for raw in ["", "   ", "Bogus+Modifier+Q", "Ctrl+Alt+"] {
            let err = parse_accelerator(&item(Some(raw))).unwrap_err();
            assert_eq!(err.code, kiri_core::error::ErrorCode::InvalidArgument, "{raw}");
        }
    }
}
