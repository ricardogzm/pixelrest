//! Brightness control through `kscreen-doctor`.
//!
//! The user's own brightness is remembered before the first change and put
//! back afterwards (the Python version always restored 100%). The remembered
//! values are also written to disk so a crash or kill can't leave a monitor
//! dimmed: they are restored on the next start.

use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};

use crate::config::state_dir;

#[derive(Default)]
pub struct Brightness {
    /// Brightness (0-100) each output had before we touched it.
    baseline: BTreeMap<String, u32>,
    /// What we last set.
    applied: BTreeMap<String, u32>,
}

impl Brightness {
    /// Restores anything left over by a previous run that didn't exit cleanly.
    pub fn recover() -> Self {
        if let Ok(text) = fs::read_to_string(state_file())
            && let Ok(saved) = serde_json::from_str::<BTreeMap<String, u32>>(&text)
            && !saved.is_empty()
        {
            log::warn!("Restoring brightness left over by a previous run: {saved:?}");
            set(saved.iter().map(|(k, v)| (k.as_str(), *v)));
        }
        let _ = fs::remove_file(state_file());
        Self::default()
    }

    /// `wanted[output] = Some(pct)` overrides an output, `None` restores it.
    pub fn apply(&mut self, wanted: &BTreeMap<String, Option<u32>>) {
        let needs_baseline: Vec<&String> = wanted
            .iter()
            .filter(|(name, v)| v.is_some() && !self.baseline.contains_key(*name))
            .map(|(name, _)| name)
            .collect();
        if !needs_baseline.is_empty() {
            let current = read();
            for name in needs_baseline {
                let pct = current.get(name).copied().unwrap_or(100);
                self.baseline.insert(name.clone(), pct);
            }
        }

        let mut changes = Vec::new();
        for (name, want) in wanted {
            match want {
                Some(pct) if self.applied.get(name) != Some(pct) => {
                    self.applied.insert(name.clone(), *pct);
                    changes.push((name.clone(), *pct));
                }
                None => {
                    if let Some(orig) = self.baseline.remove(name) {
                        self.applied.remove(name);
                        changes.push((name.clone(), orig));
                    }
                }
                _ => {}
            }
        }
        if changes.is_empty() {
            return;
        }
        self.persist();
        log::info!("Brightness: {changes:?}");
        set(changes.iter().map(|(k, v)| (k.as_str(), *v)));
    }

    pub fn restore_all(&mut self) {
        let wanted = self.baseline.keys().map(|k| (k.clone(), None)).collect();
        self.apply(&wanted);
    }

    fn persist(&self) {
        let path = state_file();
        if self.baseline.is_empty() {
            let _ = fs::remove_file(path);
            return;
        }
        let _ = fs::create_dir_all(state_dir());
        if let Err(e) = fs::write(&path, serde_json::to_string(&self.baseline).unwrap_or_default()) {
            log::warn!("Can't write {}: {e}", path.display());
        }
    }
}

fn state_file() -> PathBuf {
    state_dir().join("brightness.json")
}

fn read() -> BTreeMap<String, u32> {
    let mut map = BTreeMap::new();
    let Ok(out) = Command::new("kscreen-doctor").arg("-j").output() else { return map };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return map };
    for output in json["outputs"].as_array().into_iter().flatten() {
        if let (Some(name), Some(b)) = (output["name"].as_str(), output["brightness"].as_f64()) {
            map.insert(name.to_owned(), (b * 100.0).round().clamp(0.0, 100.0) as u32);
        }
    }
    map
}

fn set<'a>(values: impl Iterator<Item = (&'a str, u32)>) {
    let args: Vec<String> = values.map(|(name, pct)| format!("output.{name}.brightness.{pct}")).collect();
    if args.is_empty() {
        return;
    }
    match Command::new("kscreen-doctor").args(&args).output() {
        Ok(out) if !out.status.success() => {
            log::warn!("kscreen-doctor failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Err(e) => log::warn!("Can't run kscreen-doctor: {e}"),
        _ => {}
    }
}
