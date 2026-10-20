# Why a rewrite

pixelrest started as a replacement for
[esleghel/oled-saver](https://github.com/esleghel/oled-saver), a Python/PyQt6
app. On KDE Plasma (Wayland) that version crashes when monitors turn off:

```
RuntimeError: wrapped C/C++ object of type QScreen has been deleted
```

When a DisplayPort monitor powers down, KWin removes its output and Qt
deletes the `QScreen`. The app kept using it on the next blank and aborted.

pixelrest only targets Plasma on Wayland, which lets it use the compositor's
protocols directly instead of going through Qt and helper processes.

## What's different

| | oled-saver (Python) | pixelrest |
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
