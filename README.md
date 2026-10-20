# pixelrest

OLED burn-in protection for **KDE Plasma on Wayland**, written in Rust.

When you're idle, pixelrest covers your monitors with a black overlay (or
dims them) instead of turning them off, so windows stay where they are. It
waits while a video or music is playing.

It's a Plasma-only rewrite of
[esleghel/oled-saver](https://github.com/esleghel/oled-saver); see
[why a rewrite](docs/why-a-rewrite.md).

## Install

You need KDE Plasma on Wayland and a Rust toolchain (`cargo`).

```sh
git clone https://github.com/ricardogzm/pixelrest
cd pixelrest
./install.sh
```

This installs `~/.local/bin/pixelrest` and starts it now and at every login
as a systemd user service. To remove it, run `./uninstall.sh`.

## Use

pixelrest lives in the system tray. Its dot is green when active, orange
when paused, and blue while waiting for media to stop.

* **Left click:** blank now
* **Middle click:** pause / resume
* **Right click:** settings menu

Any key, click or mouse movement wakes the screens.

From a terminal:

```sh
pixelrest status
pixelrest blank | dim | wake
pixelrest toggle-pause | pause [MINUTES] | resume   # pause 0 = until resumed
pixelrest reload | quit
```

For a pause shortcut: System Settings → Keyboard → Shortcuts → Add New →
Command → `pixelrest toggle-pause`.

Logs: `journalctl --user -u pixelrest -f`

## Configure

Settings live in `~/.config/pixelrest/config.toml`. The tray menu edits the
same file. After editing it by hand, run `pixelrest reload`.

```toml
timeout_seconds = 120
idle_action = "blank"      # or "dim"
dim_brightness = 10
monitors = []              # e.g. ["DP-2"]; empty = all
pause_minutes = 30
grace_ms = 750             # ignore input right after blanking

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

## More

* [Why a rewrite](docs/why-a-rewrite.md): what was wrong with the original
  and what changed
* [How it works](docs/how-it-works.md): code layout and inhibitor handling

## License

[GPL-3.0-or-later](LICENSE), same as the original project.
