//! Wayland side and the state machine.
//!
//! * Idle detection: `ext-idle-notify-v1` straight from KWin (no swayidle, no
//!   cursor polling). The main notification honours Wayland idle inhibitors.
//! * Blanking: one opaque `wlr-layer-shell` overlay per targeted `wl_output`.
//!   Overlays are tied to output *globals*, so a monitor that powers off and
//!   disappears just loses its overlay, and gets a new one if it comes back
//!   while still blanked. This is the crash the Python version had: it kept
//!   `QScreen` objects that Qt deleted when the monitor went away.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use calloop::{
    EventLoop, LoopHandle, RegistrationToken,
    channel::{self, Sender},
    signals::{Signal, Signals},
    timer::{TimeoutAction, Timer},
};
use chrono::{Local, Timelike};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_buffer::WlBuffer, wl_keyboard, wl_output::WlOutput, wl_pointer::WlPointer, wl_seat::WlSeat, wl_shm, wl_surface::WlSurface},
};
use wayland_protocols::{
    ext::idle_notify::v1::client::{ext_idle_notification_v1, ext_idle_notifier_v1::ExtIdleNotifierV1},
    wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};

use crate::{
    brightness::Brightness,
    config::{Config, IdleAction},
    ipc, media,
    tray::{Model, Screen, Tray},
};

#[derive(Debug, Clone, Copy)]
pub enum Check {
    PowerDevil,
    Mpris,
    Audio,
}

pub enum Cmd {
    TogglePause,
    /// `None` = configured duration, `Some(0)` = until resumed.
    Pause(Option<u32>),
    Resume,
    BlankNow,
    DimNow,
    Wake,
    SetTimeout(u32),
    SetAction(IdleAction),
    /// `None` = all monitors.
    ToggleMonitor(Option<String>),
    ToggleCheck(Check),
    ToggleNight,
    OpenConfig,
    Reload,
    Quit,
    MediaResult { generation: u64, reason: Option<String> },
}

/// Which idle notification an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleKind {
    /// Fires after `timeout_seconds`; respects idle inhibitors.
    Main,
    /// Short-timeout input watcher used to undo manual blank/dim on activity.
    Watcher,
}

/// User data marker for objects whose events we handle ourselves.
struct Ud;

struct Overlay {
    output: WlOutput,
    layer: LayerSurface,
    viewport: WpViewport,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.viewport.destroy();
    }
}

enum Pause {
    Until(chrono::DateTime<Local>, RegistrationToken),
    Indefinitely,
}

pub struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    viewporter: WpViewporter,
    idle_notifier: ExtIdleNotifierV1,
    black: WlBuffer,
    _pool: RawPool,
    qh: QueueHandle<App>,
    handle: LoopHandle<'static, App>,

    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    idle_main: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
    idle_watcher: Option<ext_idle_notification_v1::ExtIdleNotificationV1>,
    overlays: Vec<Overlay>,

    config: Config,
    screen: Screen,
    shown_at: Instant,
    user_idle: bool,
    pause: Option<Pause>,
    check_generation: u64,
    retry: Option<RegistrationToken>,
    blocked: Option<String>,
    brightness: Brightness,
    night_active: bool,

    tx: Sender<Cmd>,
    tray: Option<ksni::blocking::Handle<Tray>>,
    status: Arc<Mutex<String>>,
    exit: bool,
}

pub fn run(config: Config) -> Result<()> {
    // Must exist before any thread is spawned (zbus, ksni, media checks) so
    // they all inherit the blocked signal mask and SIGTERM reaches the loop.
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP])?;

    let (tx, rx) = channel::channel::<Cmd>();
    let status = Arc::new(Mutex::new(String::new()));

    // Own the bus name first: it is our single-instance lock.
    let _bus = match ipc::serve(tx.clone(), status.clone())? {
        ipc::Serve::Owned(conn) => conn,
        ipc::Serve::AlreadyRunning => {
            log::warn!("Another instance already owns {}; exiting", ipc::BUS_NAME);
            return Ok(());
        }
    };

    let brightness = Brightness::recover();

    let conn = Connection::connect_to_env().context("connecting to the Wayland compositor")?;
    let (globals, queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor missing")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context("compositor lacks wlr-layer-shell")?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm missing")?;
    let viewporter: WpViewporter = globals.bind(&qh, 1..=1, Ud).context("compositor lacks wp_viewporter")?;
    let idle_notifier: ExtIdleNotifierV1 =
        globals.bind(&qh, 1..=2, Ud).context("compositor lacks ext-idle-notify-v1")?;

    // A single 1x1 black pixel, stretched over each output by the viewporter.
    let mut pool = RawPool::new(4, &shm)?;
    pool.mmap()[..4].copy_from_slice(&[0, 0, 0, 0]);
    let black = pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Xrgb8888, Ud, &qh);

    let mut event_loop: EventLoop<'static, App> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue).insert(handle.clone()).map_err(|e| anyhow::anyhow!("{e}"))?;

    handle
        .insert_source(rx, |event, _, app| {
            if let channel::Event::Msg(cmd) = event {
                app.handle(cmd);
            }
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    handle
        .insert_source(signals, |event, _, app| match event.signal() {
            Signal::SIGHUP => app.handle(Cmd::Reload),
            sig => {
                log::info!("Got {sig:?}, exiting");
                app.handle(Cmd::Quit);
            }
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    // Night schedule: check every minute, aligned to the minute boundary.
    handle
        .insert_source(Timer::immediate(), |_, _, app| {
            app.update_night();
            let secs = 60 - Local::now().second() as u64;
            TimeoutAction::ToDuration(Duration::from_secs(secs.max(1)))
        })
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        viewporter,
        idle_notifier,
        black,
        _pool: pool,
        qh,
        handle,
        seat: None,
        pointer: None,
        keyboard: None,
        idle_main: None,
        idle_watcher: None,
        overlays: Vec::new(),
        config,
        screen: Screen::Active,
        shown_at: Instant::now(),
        user_idle: false,
        pause: None,
        check_generation: 0,
        retry: None,
        blocked: None,
        brightness,
        night_active: false,
        tx: tx.clone(),
        tray: None,
        status,
        exit: false,
    };

    // SCTK only reports seats that appear later through `new_seat`.
    if let Some(seat) = app.seat_state.seats().next() {
        app.seat = Some(seat);
        app.reset_idle();
    }
    // Let the initial output info arrive before building the tray model.
    let deadline = Instant::now() + Duration::from_secs(1);
    while app.outputs().is_empty() && Instant::now() < deadline {
        event_loop.dispatch(Duration::from_millis(50), &mut app)?;
    }

    // Don't give up if plasmashell (the SNI host) isn't up yet at login.
    use ksni::blocking::TrayMethods;
    match Tray::new(app.model(), tx).assume_sni_available(true).spawn() {
        Ok(h) => app.tray = Some(h),
        Err(e) => log::warn!("No tray icon: {e}"),
    }
    app.refresh();
    log::info!("Running: {}", app.model().summary());

    let result = loop {
        if app.exit {
            break Ok(());
        }
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            break Err(anyhow::Error::from(e).context("event loop failed (compositor gone?)"));
        }
    };
    app.shutdown();
    result
}

impl App {
    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::TogglePause => {
                if self.pause.is_some() {
                    self.unpause();
                } else {
                    self.pause(None);
                }
            }
            Cmd::Pause(min) => self.pause(min),
            Cmd::Resume => self.unpause(),
            // Deferred a bit so the tray menu closes and the click that
            // triggered it isn't taken as activity.
            Cmd::BlankNow => self.after(150, |app| app.blank(true)),
            Cmd::DimNow => self.after(150, |app| app.dim(true)),
            Cmd::Wake => self.wake(),
            Cmd::SetTimeout(secs) => {
                self.config.timeout_seconds = secs.max(5);
                self.save();
                self.reset_idle();
            }
            Cmd::SetAction(action) => {
                self.config.idle_action = action;
                self.save();
            }
            Cmd::ToggleMonitor(name) => self.toggle_monitor(name),
            Cmd::ToggleCheck(check) => {
                let flag = match check {
                    Check::PowerDevil => &mut self.config.inhibit.powerdevil,
                    Check::Mpris => &mut self.config.inhibit.mpris,
                    Check::Audio => &mut self.config.inhibit.audio_streams,
                };
                *flag = !*flag;
                self.save();
            }
            Cmd::ToggleNight => {
                self.config.night.enabled = !self.config.night.enabled;
                self.save();
                self.update_night();
            }
            Cmd::OpenConfig => {
                match std::process::Command::new("xdg-open").arg(Config::path()).spawn() {
                    Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                    Err(e) => log::warn!("xdg-open failed: {e}"),
                }
            }
            Cmd::Reload => match Config::load() {
                Ok(cfg) => {
                    log::info!("Config reloaded");
                    self.config = cfg;
                    self.reset_idle();
                    self.sync_overlays();
                    self.update_night();
                }
                Err(e) => log::error!("Reload failed, keeping old config: {e:#}"),
            },
            Cmd::Quit => self.exit = true,
            Cmd::MediaResult { generation, reason } => self.on_media_result(generation, reason),
        }
        self.refresh();
    }

    fn after(&self, ms: u64, f: impl FnOnce(&mut App) + 'static) {
        let mut f = Some(f);
        let _ = self.handle.insert_source(Timer::from_duration(Duration::from_millis(ms)), move |_, _, app| {
            if let Some(f) = f.take() {
                f(app);
                app.refresh();
            }
            TimeoutAction::Drop
        });
    }

    fn save(&self) {
        if let Err(e) = self.config.save() {
            log::error!("Saving config failed: {e:#}");
        }
    }

    // ---- idle notifications -------------------------------------------------

    fn reset_idle(&mut self) {
        if let Some(n) = self.idle_main.take() {
            n.destroy();
        }
        self.user_idle = false;
        self.cancel_checks();
        if let Some(seat) = &self.seat {
            let ms = self.config.timeout_seconds.saturating_mul(1000);
            self.idle_main = Some(self.idle_notifier.get_idle_notification(ms, seat, &self.qh, IdleKind::Main));
            log::debug!("Idle notification armed ({} s)", self.config.timeout_seconds);
        }
    }

    fn arm_watcher(&mut self) {
        self.disarm_watcher();
        let Some(seat) = &self.seat else { return };
        // v2 can ignore inhibitors, so a playing video can't keep us dimmed.
        self.idle_watcher = Some(if self.idle_notifier.version() >= 2 {
            self.idle_notifier.get_input_idle_notification(1000, seat, &self.qh, IdleKind::Watcher)
        } else {
            self.idle_notifier.get_idle_notification(1000, seat, &self.qh, IdleKind::Watcher)
        });
    }

    fn disarm_watcher(&mut self) {
        if let Some(n) = self.idle_watcher.take() {
            n.destroy();
        }
    }

    fn on_idle_event(&mut self, kind: IdleKind, idle: bool) {
        match (kind, idle) {
            (IdleKind::Main, true) => {
                log::debug!("User idle");
                self.user_idle = true;
                self.try_idle_action();
            }
            (IdleKind::Main, false) => {
                log::debug!("User active");
                self.user_idle = false;
                self.cancel_checks();
                self.wake();
            }
            (IdleKind::Watcher, false) => self.wake(),
            (IdleKind::Watcher, true) => {}
        }
        self.refresh();
    }

    fn cancel_checks(&mut self) {
        self.check_generation += 1;
        self.blocked = None;
        if let Some(t) = self.retry.take() {
            self.handle.remove(t);
        }
    }

    fn try_idle_action(&mut self) {
        if !self.user_idle || self.pause.is_some() || self.screen != Screen::Active {
            return;
        }
        let checks = self.config.inhibit.clone();
        if !(checks.powerdevil || checks.mpris || checks.audio_streams) {
            self.idle_action();
            return;
        }
        self.check_generation += 1;
        let generation = self.check_generation;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let reason = media::blocking_reason(&checks);
            let _ = tx.send(Cmd::MediaResult { generation, reason });
        });
    }

    fn on_media_result(&mut self, generation: u64, reason: Option<String>) {
        if generation != self.check_generation || !self.user_idle || self.pause.is_some() || self.screen != Screen::Active {
            return;
        }
        match reason {
            None => {
                self.blocked = None;
                self.idle_action();
            }
            Some(why) => {
                if self.blocked.as_ref() != Some(&why) {
                    log::info!("Idle action postponed: {why}");
                }
                self.blocked = Some(why);
                let delay = Duration::from_secs(self.config.inhibit.recheck_seconds as u64);
                self.retry = self
                    .handle
                    .insert_source(Timer::from_duration(delay), |_, _, app| {
                        app.retry = None;
                        app.try_idle_action();
                        TimeoutAction::Drop
                    })
                    .ok();
            }
        }
    }

    fn idle_action(&mut self) {
        match self.config.idle_action {
            IdleAction::Blank => self.blank(false),
            IdleAction::Dim => self.dim(false),
        }
    }

    // ---- screen state ------------------------------------------------------

    fn blank(&mut self, manual: bool) {
        if self.screen == Screen::Blanked {
            return;
        }
        if self.screen == Screen::Dimmed {
            self.screen = Screen::Active;
            self.apply_brightness();
        }
        self.screen = Screen::Blanked;
        self.shown_at = Instant::now();
        if manual {
            self.arm_watcher();
        }
        self.sync_overlays();
        log::info!("Blanked {:?}", self.overlays.iter().map(|o| self.output_name(&o.output)).collect::<Vec<_>>());
    }

    fn dim(&mut self, manual: bool) {
        if self.screen != Screen::Active {
            return;
        }
        self.screen = Screen::Dimmed;
        self.shown_at = Instant::now();
        if manual {
            self.arm_watcher();
        }
        self.apply_brightness();
        log::info!("Dimmed to {}%", self.config.dim_brightness);
    }

    fn wake(&mut self) {
        self.disarm_watcher();
        match self.screen {
            Screen::Active => return,
            Screen::Blanked => self.overlays.clear(),
            Screen::Dimmed => {}
        }
        self.screen = Screen::Active;
        self.apply_brightness();
        log::info!("Woke up");
    }

    fn pause(&mut self, minutes: Option<u32>) {
        self.wake();
        self.cancel_checks();
        if let Some(Pause::Until(_, token)) = self.pause.take() {
            self.handle.remove(token);
        }
        let minutes = minutes.unwrap_or(self.config.pause_minutes);
        self.pause = Some(if minutes == 0 {
            log::info!("Paused until resumed");
            Pause::Indefinitely
        } else {
            let until = Local::now() + chrono::Duration::minutes(minutes as i64);
            let token = self
                .handle
                .insert_source(Timer::from_duration(Duration::from_secs(minutes as u64 * 60)), |_, _, app| {
                    if let Some(Pause::Until(..)) = app.pause {
                        app.pause = None; // token is being dropped by returning Drop
                        app.unpause();
                        app.refresh();
                    }
                    TimeoutAction::Drop
                })
                .expect("timer source");
            log::info!("Paused until {}", until.format("%H:%M"));
            Pause::Until(until, token)
        });
    }

    fn unpause(&mut self) {
        if let Some(Pause::Until(_, token)) = self.pause.take() {
            self.handle.remove(token);
        }
        log::info!("Resumed");
        // If the user went idle during the pause, act now.
        self.try_idle_action();
    }

    fn toggle_monitor(&mut self, name: Option<String>) {
        let all: Vec<String> = self.outputs().into_iter().map(|(n, _)| n).collect();
        match name {
            None => self.config.monitors.clear(),
            Some(name) => {
                let mut set: Vec<String> = if self.config.monitors.is_empty() { all.clone() } else { self.config.monitors.clone() };
                if let Some(i) = set.iter().position(|m| *m == name) {
                    if set.len() == 1 {
                        log::warn!("Keeping {name}: at least one monitor must stay selected");
                        return;
                    }
                    set.remove(i);
                } else {
                    set.push(name);
                }
                set.sort();
                self.config.monitors = set;
            }
        }
        self.save();
        self.sync_overlays();
        self.apply_brightness();
    }

    // ---- overlays ------------------------------------------------------------

    fn output_name(&self, output: &WlOutput) -> String {
        self.output_state.info(output).and_then(|i| i.name).unwrap_or_default()
    }

    /// (connector, description) of every live output.
    fn outputs(&self) -> Vec<(String, String)> {
        let mut v: Vec<_> = self
            .output_state
            .outputs()
            .filter_map(|o| self.output_state.info(&o))
            .filter_map(|i| {
                let desc = [i.make.as_str(), i.model.as_str()].join(" ").trim().to_owned();
                Some((i.name?, desc))
            })
            .collect();
        v.sort();
        v
    }

    /// Makes the set of overlays match the targeted, live outputs.
    fn sync_overlays(&mut self) {
        if self.screen != Screen::Blanked {
            self.overlays.clear();
            return;
        }
        let live: Vec<WlOutput> = self.output_state.outputs().collect();
        let config = &self.config;
        let state = &self.output_state;
        self.overlays.retain(|o| {
            live.contains(&o.output) && state.info(&o.output).and_then(|i| i.name).is_some_and(|n| config.targets(&n))
        });
        for output in live {
            let Some(name) = self.output_state.info(&output).and_then(|i| i.name) else { continue };
            if self.config.targets(&name) && !self.overlays.iter().any(|o| o.output == output) {
                self.create_overlay(output);
            }
        }
    }

    fn create_overlay(&mut self, output: WlOutput) {
        let surface = self.compositor.create_surface(&self.qh);
        let viewport = self.viewporter.get_viewport(&surface, &self.qh, Ud);
        let layer =
            self.layer_shell.create_layer_surface(&self.qh, surface, Layer::Overlay, Some("pixelrest"), Some(&output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        self.overlays.push(Overlay { output, layer, viewport });
    }

    fn is_overlay(&self, surface: &WlSurface) -> bool {
        self.overlays.iter().any(|o| o.layer.wl_surface() == surface)
    }

    fn input_activity(&mut self) {
        if self.screen != Screen::Active && self.shown_at.elapsed() >= Duration::from_millis(self.config.grace_ms) {
            self.wake();
            self.refresh();
        }
    }

    // ---- brightness ----------------------------------------------------------

    fn update_night(&mut self) {
        let active = self.config.night.enabled
            && self.config.night_window().is_some_and(|(start, end)| {
                let now = Local::now().time();
                if start <= end { start <= now && now < end } else { now >= start || now < end }
            });
        if active != self.night_active {
            self.night_active = active;
            log::info!("Night dimming {}", if active { "on" } else { "off" });
            self.apply_brightness();
            self.refresh();
        }
    }

    fn apply_brightness(&mut self) {
        let mut wanted = BTreeMap::new();
        for (name, _) in self.outputs() {
            let night = self.night_active.then_some(self.config.night.brightness);
            let dim = (self.screen == Screen::Dimmed && self.config.targets(&name)).then_some(self.config.dim_brightness);
            let level = match (night, dim) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            wanted.insert(name, level);
        }
        self.brightness.apply(&wanted);
    }

    fn shutdown(&mut self) {
        self.overlays.clear();
        self.screen = Screen::Active;
        self.brightness.restore_all();
        if let Some(tray) = self.tray.take() {
            tray.shutdown().wait();
        }
    }

    // ---- tray / status -------------------------------------------------------

    fn model(&self) -> Model {
        let night_label = match self.config.night_window() {
            Some(_) => format!("{}–{} at {}%", self.config.night.start, self.config.night.end, self.config.night.brightness),
            None => "(invalid times)".into(),
        };
        Model {
            screen: self.screen,
            paused: self.pause.as_ref().map(|p| match p {
                Pause::Until(t, _) => format!("until {}", t.format("%H:%M")),
                Pause::Indefinitely => "until resumed".into(),
            }),
            blocked: self.blocked.clone(),
            timeout: self.config.timeout_seconds,
            action: Some(self.config.idle_action),
            dim_brightness: self.config.dim_brightness,
            all_monitors: self.config.monitors.is_empty(),
            outputs: self.outputs().into_iter().map(|(n, d)| { let t = self.config.targets(&n); (n, d, t) }).collect(),
            powerdevil: self.config.inhibit.powerdevil,
            mpris: self.config.inhibit.mpris,
            audio: self.config.inhibit.audio_streams,
            night_enabled: self.config.night.enabled,
            night_active: self.night_active,
            night_label,
        }
    }

    fn refresh(&mut self) {
        let model = self.model();
        if let Ok(mut s) = self.status.lock() {
            *s = model.summary();
        }
        if let Some(tray) = &self.tray {
            tray.update(|t| t.model = model);
        }
    }
}

// ---- Wayland handlers ----------------------------------------------------------

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: wayland_client::protocol::wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        log::info!("Output added: {}", self.output_name(&output));
        self.sync_overlays();
        self.apply_brightness();
        self.refresh();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {
        self.sync_overlays();
        self.apply_brightness();
        self.refresh();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        log::info!("Output removed: {}", self.output_name(&output));
        self.overlays.retain(|o| o.output != output);
        // SCTK drops the output from its list after this callback returns.
        self.handle.insert_idle(|app| app.refresh());
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // The compositor closes layer surfaces whose output went away.
        self.overlays.retain(|o| o.layer != *layer);
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let Some(overlay) = self.overlays.iter().find(|o| o.layer == *layer) else { return };
        let (mut w, mut h) = configure.new_size;
        if w == 0 || h == 0 {
            let size = self.output_state.info(&overlay.output).and_then(|i| i.logical_size).unwrap_or((1, 1));
            (w, h) = (size.0.max(1) as u32, size.1.max(1) as u32);
        }
        log::debug!("Overlay on {} configured {w}x{h}", self.output_name(&overlay.output));
        let surface = overlay.layer.wl_surface();
        overlay.viewport.set_destination(w as i32, h as i32);
        surface.attach(Some(&self.black), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        overlay.layer.commit();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        if self.seat.is_none() {
            self.seat = Some(seat);
            self.reset_idle();
        }
    }

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: WlSeat, cap: Capability) {
        if self.seat.as_ref() != Some(&seat) {
            return;
        }
        match cap {
            Capability::Pointer if self.pointer.is_none() => self.pointer = self.seat_state.get_pointer(qh, &seat).ok(),
            Capability::Keyboard if self.keyboard.is_none() => self.keyboard = Some(seat.get_keyboard(qh, Ud)),
            _ => {}
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, cap: Capability) {
        match cap {
            Capability::Pointer => {
                if let Some(p) = self.pointer.take() {
                    p.release();
                }
            }
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, seat: WlSeat) {
        if self.seat.as_ref() == Some(&seat) {
            self.seat = None;
            if let Some(n) = self.idle_main.take() {
                n.destroy();
            }
            self.disarm_watcher();
        }
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, pointer: &WlPointer, events: &[PointerEvent]) {
        for event in events {
            if !self.is_overlay(&event.surface) {
                continue;
            }
            match event.kind {
                // Hide the cursor over the overlay.
                PointerEventKind::Enter { serial } => pointer.set_cursor(serial, None, 0, 0),
                PointerEventKind::Leave { .. } => {}
                _ => self.input_activity(),
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch2<ext_idle_notification_v1::ExtIdleNotificationV1, App> for IdleKind {
    fn event(
        &self,
        app: &mut App,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => app.on_idle_event(*self, true),
            ext_idle_notification_v1::Event::Resumed => app.on_idle_event(*self, false),
            _ => {}
        }
    }
}

impl Dispatch2<wl_keyboard::WlKeyboard, App> for Ud {
    fn event(&self, app: &mut App, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &Connection, _: &QueueHandle<App>) {
        // Keymap fds are closed when the event (and its OwnedFd) is dropped.
        if let wl_keyboard::Event::Key { state: wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed), .. } = event {
            app.input_activity();
        }
    }
}

/// Objects without events we care about.
macro_rules! ignore_events {
    ($($ty:ty),*) => {$(
        impl Dispatch2<$ty, App> for Ud {
            fn event(&self, _: &mut App, _: &$ty, _: <$ty as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
        }
    )*};
}
ignore_events!(WlBuffer, WpViewporter, WpViewport, ExtIdleNotifierV1);

delegate_registry!(App);
delegate_dispatch2!(App);
