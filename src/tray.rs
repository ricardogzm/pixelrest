//! StatusNotifierItem tray icon (native to the Plasma panel).

use calloop::channel::Sender;
use ksni::{
    Icon, MenuItem, ToolTip,
    menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu},
};

use crate::{
    app::{Check, Cmd},
    config::IdleAction,
};

const TIMEOUT_PRESETS: [u32; 8] = [30, 60, 120, 300, 600, 900, 1800, 3600];
const PAUSE_PRESETS: [u32; 4] = [15, 30, 60, 120];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Active,
    Blanked,
    Dimmed,
}

#[derive(Debug, Clone, Default)]
pub struct Model {
    pub screen: Screen,
    /// e.g. "until 14:30"
    pub paused: Option<String>,
    pub blocked: Option<String>,
    pub timeout: u32,
    pub action: Option<IdleAction>,
    pub dim_brightness: u32,
    pub all_monitors: bool,
    /// (connector, description, targeted)
    pub outputs: Vec<(String, String, bool)>,
    pub powerdevil: bool,
    pub mpris: bool,
    pub audio: bool,
    pub night_enabled: bool,
    pub night_active: bool,
    pub night_label: String,
}

impl Model {
    pub fn summary(&self) -> String {
        let state = match (&self.paused, self.screen) {
            (Some(p), _) => format!("Paused {p}"),
            (None, Screen::Blanked) => "Blanked".into(),
            (None, Screen::Dimmed) => "Dimmed".into(),
            (None, Screen::Active) => match &self.blocked {
                Some(why) => format!("Waiting: {why}"),
                None => format!("Active, {} idle timeout", fmt_secs(self.timeout)),
            },
        };
        let targets: Vec<&str> = self.outputs.iter().filter(|o| o.2).map(|o| o.0.as_str()).collect();
        format!("{state} [{}]", targets.join(", "))
    }
}

pub struct Tray {
    pub model: Model,
    tx: Sender<Cmd>,
}

impl Tray {
    pub fn new(model: Model, tx: Sender<Cmd>) -> Self {
        Self { model, tx }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

fn item(label: impl Into<String>, icon: &str, cmd: impl Fn() -> Cmd + Send + 'static) -> MenuItem<Tray> {
    StandardItem {
        label: label.into(),
        icon_name: icon.into(),
        activate: Box::new(move |t: &mut Tray| t.send(cmd())),
        ..Default::default()
    }
    .into()
}

fn check(label: impl Into<String>, checked: bool, cmd: impl Fn() -> Cmd + Send + 'static) -> MenuItem<Tray> {
    CheckmarkItem {
        label: label.into(),
        checked,
        activate: Box::new(move |t: &mut Tray| t.send(cmd())),
        ..Default::default()
    }
    .into()
}

fn label(text: impl Into<String>) -> MenuItem<Tray> {
    StandardItem { label: text.into(), enabled: false, ..Default::default() }.into()
}

fn submenu(label: impl Into<String>, icon: &str, items: Vec<MenuItem<Tray>>) -> MenuItem<Tray> {
    SubMenu { label: label.into(), icon_name: icon.into(), submenu: items, ..Default::default() }.into()
}

pub fn fmt_secs(s: u32) -> String {
    match s {
        s if s % 3600 == 0 => format!("{} h", s / 3600),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "pixelrest".into()
    }

    fn title(&self) -> String {
        "PixelRest".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let dot = match (&self.model.paused, &self.model.blocked) {
            (Some(_), _) => [0xff, 0xa5, 0x00],
            (None, Some(_)) => [0x3d, 0xae, 0xe9],
            (None, None) => [0x2e, 0xcc, 0x71],
        };
        [22, 32, 44, 64].into_iter().map(|s| draw_icon(s, dot)).collect()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "PixelRest".into(), description: self.model.summary(), ..Default::default() }
    }

    /// Left click: blank now.
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::BlankNow);
    }

    /// Middle click: toggle pause.
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::TogglePause);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let m = &self.model;
        let mut menu = vec![
            label(m.summary()),
            MenuItem::Separator,
            item("Blank now", "video-display", || Cmd::BlankNow),
            item(format!("Dim now ({}%)", m.dim_brightness), "brightness-low", || Cmd::DimNow),
            MenuItem::Separator,
        ];

        if let Some(p) = &m.paused {
            menu.push(item(format!("Resume (paused {p})"), "media-playback-start", || Cmd::Resume));
        } else {
            let mut items: Vec<_> = PAUSE_PRESETS
                .iter()
                .map(|&min| item(fmt_secs(min * 60), "", move || Cmd::Pause(Some(min))))
                .collect();
            items.push(item("Until resumed", "", || Cmd::Pause(Some(0))));
            menu.push(submenu("Pause", "media-playback-pause", items));
        }
        menu.push(MenuItem::Separator);

        let mut presets = TIMEOUT_PRESETS.to_vec();
        if !presets.contains(&m.timeout) {
            presets.push(m.timeout);
            presets.sort_unstable();
        }
        let selected = presets.iter().position(|&p| p == m.timeout).unwrap_or(0);
        let options = presets.iter().map(|&p| RadioItem { label: fmt_secs(p), ..Default::default() }).collect();
        menu.push(submenu(
            format!("Idle timeout: {}", fmt_secs(m.timeout)),
            "chronometer",
            vec![
                RadioGroup {
                    selected,
                    select: Box::new(move |t: &mut Tray, i| t.send(Cmd::SetTimeout(presets[i]))),
                    options,
                }
                .into(),
            ],
        ));

        let action = m.action.unwrap_or(IdleAction::Blank);
        menu.push(submenu(
            "When idle",
            "configure",
            vec![
                RadioGroup {
                    selected: (action == IdleAction::Dim) as usize,
                    select: Box::new(|t: &mut Tray, i| {
                        t.send(Cmd::SetAction(if i == 0 { IdleAction::Blank } else { IdleAction::Dim }))
                    }),
                    options: vec![
                        RadioItem { label: "Blank (black overlay)".into(), ..Default::default() },
                        RadioItem { label: format!("Dim to {}%", m.dim_brightness), ..Default::default() },
                    ],
                }
                .into(),
            ],
        ));

        let mut monitors = vec![check("All monitors", m.all_monitors, || Cmd::ToggleMonitor(None)), MenuItem::Separator];
        for (name, desc, on) in &m.outputs {
            let n = name.clone();
            let text = if desc.is_empty() { name.clone() } else { format!("{name} — {desc}") };
            monitors.push(check(text, *on, move || Cmd::ToggleMonitor(Some(n.clone()))));
        }
        menu.push(submenu("Monitors", "monitor", monitors));

        menu.push(submenu(
            "Don't blank while",
            "media-playback-start",
            vec![
                check("Media is playing (MPRIS)", m.mpris, || Cmd::ToggleCheck(Check::Mpris)),
                check("An app inhibits the screensaver", m.powerdevil, || Cmd::ToggleCheck(Check::PowerDevil)),
                check("Any audio stream is open", m.audio, || Cmd::ToggleCheck(Check::Audio)),
            ],
        ));

        let night = if m.night_active { format!("{} (active)", m.night_label) } else { m.night_label.clone() };
        menu.push(check(format!("Night dimming {night}"), m.night_enabled, || Cmd::ToggleNight));

        menu.extend([
            MenuItem::Separator,
            item("Edit config file", "document-edit", || Cmd::OpenConfig),
            item("Reload config", "view-refresh", || Cmd::Reload),
            item("Quit", "application-exit", || Cmd::Quit),
        ]);
        menu
    }
}

/// Monitor outline with a status dot, rendered with simple SDF anti-aliasing.
fn draw_icon(size: i32, dot: [u8; 3]) -> Icon {
    let s = size as f32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let rrect = |px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
        let (dx, dy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
        (dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0)) - r
    };
    let cov = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let stroke = s * 0.085;
            let frame = rrect(px, py, s * 0.06, s * 0.12, s * 0.94, s * 0.74, s * 0.09);
            let outline = cov(frame.abs() - stroke / 2.0);
            let inside = cov(frame);
            let stand = cov(rrect(px, py, s * 0.42, s * 0.72, s * 0.58, s * 0.86, 0.0))
                .max(cov(rrect(px, py, s * 0.26, s * 0.84, s * 0.74, s * 0.92, s * 0.03)));
            let d_dot = (px - s * 0.76).hypot(py - s * 0.72) - s * 0.2;
            let dot_ring = cov(d_dot - s * 0.06);
            let dot_fill = cov(d_dot);

            // Layer: black screen, light outline/stand, dark ring, colored dot.
            let mut px_rgba = [0f32; 4];
            let mut over = |c: [f32; 3], a: f32| {
                let out_a = a + px_rgba[3] * (1.0 - a);
                if out_a > 0.0 {
                    for i in 0..3 {
                        px_rgba[i] = (c[i] * a + px_rgba[i] * px_rgba[3] * (1.0 - a)) / out_a;
                    }
                }
                px_rgba[3] = out_a;
            };
            over([0.0, 0.0, 0.0], inside * 0.9);
            over([0.9, 0.9, 0.9], outline.max(stand));
            over([0.1, 0.1, 0.1], dot_ring);
            over([dot[0] as f32 / 255.0, dot[1] as f32 / 255.0, dot[2] as f32 / 255.0], dot_fill);

            let b = |v: f32| (v * 255.0).round() as u8;
            data.extend([b(px_rgba[3]), b(px_rgba[0]), b(px_rgba[1]), b(px_rgba[2])]);
        }
    }
    Icon { width: size, height: size, data }
}
