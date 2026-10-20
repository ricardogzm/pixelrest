//! Session-bus control interface. Owning the bus name also makes the daemon
//! single-instance, and the CLI subcommands are thin clients of it (handy for
//! KDE global shortcuts).

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use calloop::channel::Sender;
use zbus::blocking::{Connection, connection};

use crate::app::Cmd;

pub const BUS_NAME: &str = "dev.ricardogzm.PixelRest";
const PATH: &str = "/dev/ricardogzm/PixelRest";
const IFACE: &str = "dev.ricardogzm.PixelRest";

struct Control {
    tx: Sender<Cmd>,
    status: Arc<Mutex<String>>,
}

impl Control {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

#[zbus::interface(name = "dev.ricardogzm.PixelRest")]
impl Control {
    fn toggle_pause(&self) {
        self.send(Cmd::TogglePause);
    }
    /// 0 = configured default duration.
    fn pause(&self, minutes: u32) {
        self.send(Cmd::Pause((minutes > 0).then_some(minutes)));
    }
    fn resume(&self) {
        self.send(Cmd::Resume);
    }
    fn blank(&self) {
        self.send(Cmd::BlankNow);
    }
    fn dim(&self) {
        self.send(Cmd::DimNow);
    }
    fn wake(&self) {
        self.send(Cmd::Wake);
    }
    fn reload(&self) {
        self.send(Cmd::Reload);
    }
    fn quit(&self) {
        self.send(Cmd::Quit);
    }
    fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

pub enum Serve {
    Owned(Connection),
    AlreadyRunning,
}

pub fn serve(tx: Sender<Cmd>, status: Arc<Mutex<String>>) -> Result<Serve> {
    let built = connection::Builder::session()?
        .name(BUS_NAME)?
        // zbus defaults to letting a newer instance steal the name.
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .serve_at(PATH, Control { tx, status })?
        .build();
    match built {
        Ok(conn) => Ok(Serve::Owned(conn)),
        Err(zbus::Error::NameTaken) => Ok(Serve::AlreadyRunning),
        Err(e) => Err(e).context("registering on the session bus"),
    }
}

/// Calls `method` on the running daemon; returns its string reply, if any.
pub fn call(method: &str, minutes: Option<u32>) -> Result<Option<String>> {
    let conn = Connection::session()?;
    let reply = match minutes {
        Some(m) => conn.call_method(Some(BUS_NAME), PATH, Some(IFACE), method, &m),
        None => conn.call_method(Some(BUS_NAME), PATH, Some(IFACE), method, &()),
    };
    let reply = reply.map_err(|e| match e {
        zbus::Error::MethodError(name, ..) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" => {
            anyhow::anyhow!("pixelrest is not running")
        }
        e => e.into(),
    })?;
    Ok(reply.body().deserialize::<String>().ok())
}
