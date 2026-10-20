use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use chrono::NaiveTime;
use serde::{Deserialize, Serialize};

const HEADER: &str = "\
# pixelrest configuration.
# Edited by the tray menu too; reload with `pixelrest reload` or the tray menu.
#
# timeout_seconds  seconds without input before the idle action runs
# idle_action      \"blank\" (black overlay) or \"dim\" (lower brightness)
# monitors         connector names to protect, e.g. [\"DP-2\"]; empty = all
# pause_minutes    default length of \"Pause\"
# grace_ms         ignore input for this long after blanking (avoids instant wake)
#
# [inhibit]        don't run the idle action while...
#   powerdevil     an app holds a KDE/freedesktop screensaver inhibition
#   mpris          an MPRIS media player reports \"Playing\"
#   audio_streams  any uncorked PipeWire/PulseAudio stream exists (noisy)
#   ignore_players MPRIS bus-name substrings to ignore, e.g. [\"spotify\"]
#   recheck_seconds  how often to re-check while blocked
# Apps using the Wayland idle-inhibit protocol are always respected.
#
# [night]          dim every monitor on a schedule
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdleAction {
    Blank,
    Dim,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub timeout_seconds: u32,
    pub idle_action: IdleAction,
    pub dim_brightness: u32,
    pub monitors: Vec<String>,
    pub pause_minutes: u32,
    pub grace_ms: u64,
    pub inhibit: Inhibit,
    pub night: Night,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Inhibit {
    pub powerdevil: bool,
    pub mpris: bool,
    pub audio_streams: bool,
    pub ignore_players: Vec<String>,
    pub recheck_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Night {
    pub enabled: bool,
    pub start: String,
    pub end: String,
    pub brightness: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout_seconds: 120,
            idle_action: IdleAction::Blank,
            dim_brightness: 10,
            monitors: Vec::new(),
            pause_minutes: 30,
            grace_ms: 750,
            inhibit: Inhibit::default(),
            night: Night::default(),
        }
    }
}

impl Default for Inhibit {
    fn default() -> Self {
        Self {
            powerdevil: true,
            mpris: true,
            audio_streams: false,
            ignore_players: Vec::new(),
            recheck_seconds: 30,
        }
    }
}

impl Default for Night {
    fn default() -> Self {
        Self {
            enabled: false,
            start: "22:00".into(),
            end: "07:00".into(),
            brightness: 20,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("pixelrest").join("config.toml")
    }

    /// Loads the config, creating it with defaults on first run.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            let cfg = Config::default();
            cfg.save()?;
            log::info!("Created {}", path.display());
            return Ok(cfg);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.sanitize();
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let body = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, format!("{HEADER}\n{body}"))?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    fn sanitize(&mut self) {
        self.timeout_seconds = self.timeout_seconds.clamp(5, 24 * 3600);
        self.dim_brightness = self.dim_brightness.clamp(0, 100);
        self.night.brightness = self.night.brightness.clamp(0, 100);
        self.pause_minutes = self.pause_minutes.max(1);
        self.inhibit.recheck_seconds = self.inhibit.recheck_seconds.max(5);
        self.grace_ms = self.grace_ms.min(10_000);
    }

    pub fn targets(&self, output: &str) -> bool {
        self.monitors.is_empty() || self.monitors.iter().any(|m| m == output)
    }

    pub fn night_window(&self) -> Option<(NaiveTime, NaiveTime)> {
        let parse = |s: &str| NaiveTime::parse_from_str(s.trim(), "%H:%M").ok();
        Some((parse(&self.night.start)?, parse(&self.night.end)?))
    }
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/state"))
        .join("pixelrest")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}
