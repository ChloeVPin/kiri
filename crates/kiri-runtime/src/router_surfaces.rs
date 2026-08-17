//! Maps a control-plane command id to the host surface that owns it.
//!
//! Used by the Windows (and later cross) lazy Router so plugin construction
//! happens on first `window.kiri.send()` for that surface, not before
//! WebView2 environment creation.

use kiri_core::dispatch::command_id;

/// One attachable control-plane surface. `Core` is ping/diag/resources/plugin.list
/// via the plugin ABI; everything else is a `Router::with_*` builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    Core,
    Platform,
    Fs,
    Window,
    Clipboard,
    Path,
    Http,
    Shell,
    Notification,
    Dialog,
    Shortcut,
    Autostart,
    Store,
    Deeplink,
    Opener,
    WindowState,
    Tray,
    Sidecar,
    Event,
    Config,
    Updater,
    Cli,
    FsWatch,
    Ws,
    Menu,
}

/// Which surface must be attached before `command_id` can dispatch.
pub(crate) fn surface_for_command(id: u32) -> Option<Surface> {
    use command_id::*;
    Some(match id {
        PING | DIAGNOSTICS | RESOURCES_OPEN | RESOURCES_CLOSE | PLUGIN_LIST => Surface::Core,
        PLATFORM_OS | PLATFORM_ARCH | APP_VERSION | EVENT_EMIT | EVENT_LISTEN => Surface::Platform,
        FS_READ | FS_WRITE | FS_EXISTS | FS_REMOVE => Surface::Fs,
        WINDOW_TITLE_GET | WINDOW_TITLE_SET | WINDOW_SHOW | WINDOW_HIDE | WINDOW_MINIMIZE
        | WINDOW_MAXIMIZE | WINDOW_RESTORE | WINDOW_CLOSE | WINDOW_FOCUS => Surface::Window,
        CLIPBOARD_READ | CLIPBOARD_WRITE => Surface::Clipboard,
        PATH_DIRNAME | PATH_BASENAME | PATH_EXTNAME | PATH_STEM | PATH_JOIN | PATH_IS_ABSOLUTE
        | OS_HOME_DIR | OS_TEMP_DIR | OS_APP_CONFIG_DIR | OS_APP_DATA_DIR | OS_APP_CACHE_DIR
        | OS_DOCUMENT_DIR | OS_APP_DIR => Surface::Path,
        HTTP_GET | HTTP_POST | HTTP_PUT | HTTP_PATCH | HTTP_DELETE => Surface::Http,
        SHELL_RUN => Surface::Shell,
        NOTIFY => Surface::Notification,
        DIALOG_OPEN => Surface::Dialog,
        SHORTCUT_REGISTER => Surface::Shortcut,
        AUTOSTART_SET | AUTOSTART_GET => Surface::Autostart,
        STORE_GET | STORE_SET => Surface::Store,
        DEEPLINK_REGISTER => Surface::Deeplink,
        OPENER_OPEN => Surface::Opener,
        WINDOW_STATE_SAVE | WINDOW_STATE_LOAD => Surface::WindowState,
        TRAY_SET_MENU | TRAY_INVOKE => Surface::Tray,
        SIDECAR_SPAWN | SIDECAR_STOP | SIDECAR_LIST => Surface::Sidecar,
        EVENT_PUBLISH | EVENT_SUBSCRIBE | EVENT_CHANNELS => Surface::Event,
        CONFIG_GET | CONFIG_KEYS => Surface::Config,
        UPDATER_CHECK => Surface::Updater,
        CLI_ARGS => Surface::Cli,
        FS_WATCH | FS_UNWATCH => Surface::FsWatch,
        WS_CONNECT | WS_SEND | WS_CLOSE => Surface::Ws,
        MENU_SET | MENU_INVOKE => Surface::Menu,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_command_maps_to_a_surface() {
        let mut missing = Vec::new();
        for cmd in kiri_core::commands::COMMANDS {
            if surface_for_command(cmd.id).is_none() {
                missing.push((cmd.id, cmd.name));
            }
        }
        assert!(missing.is_empty(), "unmapped catalog commands: {missing:?}");
    }

    #[test]
    fn ping_is_core_so_ipc_bench_does_not_load_plugins() {
        assert_eq!(surface_for_command(command_id::PING), Some(Surface::Core));
        assert_eq!(surface_for_command(command_id::TRAY_SET_MENU), Some(Surface::Tray));
        assert_eq!(surface_for_command(command_id::SIDECAR_SPAWN), Some(Surface::Sidecar));
    }
}
