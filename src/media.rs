//! Checks for things that should postpone the idle action: KDE/freedesktop
//! screensaver inhibitions, playing MPRIS players and live audio streams.
//!
//! Wayland idle-inhibit is handled by the compositor itself (the idle
//! notification simply never fires), so it isn't checked here.

use std::{process::Command, time::Duration};

use zbus::blocking::{Connection, Proxy, connection};

use crate::config::Inhibit;

/// Returns a human readable reason when the idle action should wait.
pub fn blocking_reason(cfg: &Inhibit) -> Option<String> {
    if cfg.powerdevil || cfg.mpris {
        match session() {
            Ok(conn) => {
                if cfg.powerdevil && powerdevil_inhibited(&conn) {
                    return Some("an app is inhibiting the screensaver".into());
                }
                if cfg.mpris
                    && let Some(player) = playing_player(&conn, &cfg.ignore_players)
                {
                    return Some(format!("{player} is playing"));
                }
            }
            Err(e) => log::warn!("D-Bus session unavailable: {e}"),
        }
    }
    if cfg.audio_streams
        && let Some(app) = active_audio_stream()
    {
        return Some(format!("{app} is playing audio"));
    }
    None
}

fn session() -> zbus::Result<Connection> {
    connection::Builder::session()?.method_timeout(Duration::from_secs(2)).build()
}

fn powerdevil_inhibited(conn: &Connection) -> bool {
    const CHANGE_SCREEN_SETTINGS: u32 = 4;
    let reply = conn.call_method(
        Some("org.kde.Solid.PowerManagement.PolicyAgent"),
        "/org/kde/Solid/PowerManagement/PolicyAgent",
        Some("org.kde.Solid.PowerManagement.PolicyAgent"),
        "HasInhibition",
        &CHANGE_SCREEN_SETTINGS,
    );
    match reply.and_then(|m| m.body().deserialize::<bool>()) {
        Ok(inhibited) => inhibited,
        Err(e) => {
            log::debug!("PowerDevil HasInhibition failed: {e}");
            false
        }
    }
}

fn playing_player(conn: &Connection, ignore: &[String]) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(conn).ok()?;
    let names = dbus.list_names().ok()?;
    for name in names {
        let name = name.as_str();
        let Some(short) = name.strip_prefix("org.mpris.MediaPlayer2.") else { continue };
        let lower = short.to_ascii_lowercase();
        if ignore.iter().any(|i| lower.contains(&i.to_ascii_lowercase())) {
            continue;
        }
        let status: Option<String> = Proxy::new(conn, name, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player")
            .and_then(|p| p.get_property("PlaybackStatus"))
            .ok();
        if status.as_deref() == Some("Playing") {
            return Some(short.split('.').next().unwrap_or(short).to_owned());
        }
    }
    None
}

fn active_audio_stream() -> Option<String> {
    let out = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).output().ok()?;
    let streams: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    streams.iter().find(|s| s["corked"] == false).map(|s| {
        s["properties"]["application.name"].as_str().unwrap_or("an app").to_owned()
    })
}
