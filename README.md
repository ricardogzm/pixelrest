# pixelrest

OLED burn-in protection for **KDE Plasma on Wayland**, written in Rust.
It is a rewrite of [esleghel/oled-saver](https://github.com/esleghel/oled-saver)
that only targets Plasma/Wayland.

When you're idle it covers the chosen monitors with a black overlay (or
dims them) without powering them off, so windows stay where they are. It
waits while a video or music is playing.

## Why a rewrite

The Python version crashes when monitors turn off:

```
RuntimeError: wrapped C/C++ object of type QScreen has been deleted
```

When a DisplayPort monitor powers down, KWin removes its output and Qt
deletes the `QScreen`. The app kept using it on the next blank and aborted.

What's different here:

| | Python version | This version |
|---|---|---|
| Monitor turns off / hotplug | crash | the overlay is dropped with the output and recreated if it comes back |
| Idle detection | spawns `swayidle`, sends Unix signals back | `ext-idle-notify-v1` straight from KWin |
| Blank window | Qt fullscreen window | `wlr-layer-shell` overlay layer, above panels, cursor hidden, keyboard grabbed |
| Video players (Wayland idle-inhibit) | not seen | respected natively |
| Brightness after dimming | always reset to 100% | restored to your value, also after a crash |
| Waking from a manual dim | polls `QCursor` (doesn't work on Wayland) | input watcher from the compositor |
| Single instance / control | PID file in `/tmp` + signals | D-Bus name + `pixelrest <command>` |
| Autostart | XDG autostart | systemd user service, restarts on failure |
| Runtime | Python + PyQt6, about 70 MB | one ~4.5 MB binary |

## Install

```sh
./install.sh
```

The script builds a release binary, installs it to `~/.local/bin/pixelrest`
and enables `pixelrest.service` (a systemd user unit). `./uninstall.sh`
removes both. The config is created with defaults on first run.

Logs: `journalctl --user -u pixelrest -f`

## Use

The tray icon's dot shows the state: green means active, orange paused,
blue waiting because media is playing.

* **Left click:** blank now
* **Middle click:** toggle pause
* **Right click:** menu (pause, idle timeout, blank/dim, monitors, media exceptions, night dimming)

Any key, click or mouse movement wakes the screens. Input in the first
`grace_ms` after blanking is ignored, so the click that triggered it doesn't
wake it straight away.

CLI (talks to the running daemon over D-Bus):

```sh
pixelrest status
pixelrest blank | dim | wake
pixelrest toggle-pause | pause [MINUTES] | resume   # pause 0 = until resumed
pixelrest reload | quit
```

To make a global shortcut (like the old Super+B): System Settings →
Keyboard → Shortcuts → Add New → Command → `pixelrest toggle-pause`.

## Config

`~/.config/pixelrest/config.toml`. The tray menu also edits this file.
Reload it with `pixelrest reload` or `systemctl --user reload-or-restart pixelrest`.

```toml
timeout_seconds = 120
idle_action = "blank"      # or "dim"
dim_brightness = 10
monitors = []              # e.g. ["DP-2"]; empty = all
pause_minutes = 30
grace_ms = 750

[inhibit]                  # don't act while...
powerdevil = true          # an app inhibits the screensaver (browsers playing video, etc.)
mpris = true               # an MPRIS player is Playing
audio_streams = false      # any open audio stream (very noisy: Discord, games, ...)
ignore_players = []        # e.g. ["spotify"] to blank even while Spotify plays
recheck_seconds = 30

[night]                    # dim every monitor on a schedule
enabled = false
start = "22:00"
end = "07:00"
brightness = 20
```

Apps that use the Wayland idle-inhibit protocol (mpv, games via SDL, …)
always prevent blanking, because KWin doesn't report you as idle while
they're active.

## How it works

* `src/app.rs` holds the Wayland state, the overlays per `wl_output`, the
  idle notifications and the state machine.
* `src/media.rs` runs the PowerDevil, MPRIS and PipeWire checks on a worker
  thread.
* `src/brightness.rs` drives `kscreen-doctor` and keeps the saved brightness
  in `~/.local/state/pixelrest/brightness.json`.
* `src/tray.rs` is the StatusNotifierItem tray (via `ksni`, no Qt or GTK).
* `src/ipc.rs` is the `dev.ricardogzm.PixelRest` session-bus interface.

## License

[GPL-3.0-or-later](LICENSE), same as the original project.
