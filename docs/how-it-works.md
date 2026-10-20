# How it works

* `src/app.rs` holds the Wayland state, the overlays per `wl_output`, the
  idle notifications and the state machine.
* `src/media.rs` runs the PowerDevil, MPRIS and PipeWire checks on a worker
  thread.
* `src/brightness.rs` drives `kscreen-doctor` and keeps the saved brightness
  in `~/.local/state/pixelrest/brightness.json`.
* `src/tray.rs` is the StatusNotifierItem tray (via `ksni`, no Qt or GTK).
* `src/ipc.rs` is the `dev.ricardogzm.PixelRest` session-bus interface.

Apps that use the Wayland idle-inhibit protocol (mpv, games via SDL, …)
always prevent blanking, because KWin doesn't report you as idle while
they're active. Other inhibitions (browsers, PowerDevil, MPRIS players) are
checked when the idle timeout fires, and re-checked every
`recheck_seconds` while they block.
