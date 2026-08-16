# my-mouseless

Keyboard-driven cursor control for Windows and Wayland, in Rust. Press a hotkey,
a labelled grid covers every monitor, type the label, the cursor goes there and
clicks.

Status: **working vertical slice.** Grid selection, refinement, cursor mode,
left/right/middle clicking, and drag (text selection) all work end to end on both
platforms. No tray icon, no settings UI, no scrolling and no double click yet.

The Wayland backend is developed and tested against Hyprland. It needs
`wlr-layer-shell` and `wlr-virtual-pointer`, which most wlroots-based
compositors implement — see [Wayland notes](#wayland-notes) for what differs and
why.

## Build and run

```bash
cargo run --release
```

First run writes the config file with the defaults below:
`%APPDATA%\my-mouseless\config.toml` on Windows,
`$XDG_CONFIG_HOME/my-mouseless/config.toml` (usually `~/.config/…`) on Linux.

Windows is then finished: the program installs its own hotkey. Linux needs three
more minutes, below.

## Linux setup

**The `hotkey` setting in `config.toml` does nothing here.** No Wayland client
can watch for a key it does not own — the compositor holds that key,
deliberately, and no setting in this program can take it. So the program runs as
a daemon and the compositor pokes it. Three pieces: put the binary on `PATH`,
bind a key to it, start it at login.

### 1. Put the binary on `PATH`

A symlink rather than a copy, so `cargo build --release` updates it with no
reinstall step:

```bash
mkdir -p ~/.local/bin
ln -sf "$PWD/target/release/my-mouseless" ~/.local/bin/my-mouseless
```

Check your compositor can see it — this is the PATH that graphical sessions get,
which is not always your shell's:

```bash
systemctl --user show-environment | grep -E '^PATH=' | tr ':' '\n' | grep local/bin
```

If nothing prints, use the full path in the two config snippets below instead of
the bare `my-mouseless`.

### 2. Bind a key

**Omarchy** configures Hyprland in Lua. In `~/.config/hypr/bindings.lua`:

```lua
o.bind("CTRL + ALT + SPACE", "Mouseless grid", "my-mouseless toggle")
```

**Stock Hyprland**, in `~/.config/hypr/hyprland.conf`:

```
bind = CTRL ALT, SPACE, exec, my-mouseless toggle
```

**Sway**, in `~/.config/sway/config`:

```
bindsym Ctrl+Alt+space exec my-mouseless toggle
```

Pick any key you like — `Ctrl+Alt+Space` only matches the Windows default so the
two feel the same. Check it is free first, and unbind it if not: on Omarchy,
`omarchy menu keybindings --print` lists everything, and rebinding an occupied
key needs an `hl.unbind("...")` line above your `o.bind`. Omarchy leaves
`Ctrl+Alt+Space` free.

### 3. Start it at login

**Omarchy**, in `~/.config/hypr/autostart.lua`:

```lua
o.launch_on_start("my-mouseless")
```

**Stock Hyprland**, in `~/.config/hypr/hyprland.conf`:

```
exec-once = my-mouseless
```

That only fires at login, so start it once by hand now — or just log out and
back in:

```bash
my-mouseless &
```

### 4. Check it

```bash
hyprctl reload && hyprctl configerrors   # Omarchy / Hyprland: must print nothing
pgrep -x my-mouseless                    # the daemon is up
my-mouseless toggle                      # the grid should appear
```

If `toggle` works but your key does not, the binding is the problem, not this
program. If neither works, read the daemon's output — it prints what it bound
and what it found at startup.

### What the commands do

`my-mouseless toggle` opens or closes the grid on the running daemon;
`my-mouseless quit` shuts it down. Both talk to it over
`$XDG_RUNTIME_DIR/my-mouseless.sock`, so they cost a process spawn and nothing
else. Starting a second daemon is refused rather than allowed to fight the first
one over the cursor.

Nothing needs root, `uinput`, or membership of the `input` group: the cursor is
driven through `wlr-virtual-pointer`, which is an ordinary Wayland protocol.

## Using it

Two phases. The grid gets you close, then cursor mode lets you aim and choose a
button.

**Grid** — three keystrokes: two for the coarse cell, one for the refinement.

| Key | Action |
| --- | --- |
| the trigger | show / hide the grid (`Ctrl+Alt+Space` on Windows; your compositor binding on Wayland) |
| label characters | narrow the selection |
| `Backspace` | undo one character |
| `Esc` | cancel |

**Cursor mode** — the grid is replaced by a crosshair at the cursor, with the
keys listed on screen.

| Key | Action |
| --- | --- |
| `Space` / `Enter` | left click |
| `r` | right click |
| `m` | middle click |
| `v` | start / finish a drag |
| `g` | re-open the grid, keeping any drag |
| arrows or `hjkl` | move 1 px |
| `Shift` + the above | move 16 px |
| `Esc` | leave without clicking |

Clicking exits and releases the keyboard.

## Selecting text

Text selection is a drag: press at the start, move, release at the end. `v`
anchors the drag and `g` re-opens the grid without losing it, so you can cross a
long selection by picking its end point instead of nudging there a pixel at a
time.

1. the trigger, then the label — cursor lands near the start of the text
2. nudge with `hjkl` to the exact character
3. `v` — the crosshair turns amber and reads `DRAGGING`
4. `g`, then a label — jump to the end of the selection
5. nudge to the exact end
6. `Space` — the drag is performed, text is selected

For a short selection, skip steps 4–5 and just nudge: `Shift`+`hjkl` moves 16 px
at a time.

On **Windows** the left button is physically held from step 3, so the selection
highlights as you travel. On **Wayland** it cannot be (see below), so the whole
press-travel-release happens at step 6 and the selection appears at the end. Same
result, less feedback on the way.

Double-click-to-select-a-word is not implemented yet; there is no double click.

**The button is never left held.** Every exit — `Space`, `v`, `Esc`, the trigger,
a config reload — releases it; on Windows `Ctrl+C` releases it through a console
control handler, and on Linux `SIGINT`/`SIGTERM` are turned into a clean
shutdown for the same reason. This matters because a synthetic button-down that
never gets its matching up cannot be cleared by the user: they never physically
pressed it. There is a test asserting every exit path releases.

A third grid level was tried and removed: it put cells at roughly 6x8 px, where
the labels are too small to read. Precision past two levels comes from nudging
instead, which is also what makes right and middle click reachable.

Set `click_on_select = true` to skip cursor mode and left-click the moment the
grid selection completes. It is faster, but at two levels the cell center can be
~18 px off target, so it misses small controls — and because it never enters
cursor mode, it also disables dragging and right/middle click entirely.

## Configuration

```toml
hotkey = "ctrl+alt+space"   # Windows only; on Wayland the compositor binds it
tap_timeout_ms = 250
double_tap_ms = 350
coarse_cols = 24
coarse_rows = 14
refine_cols = 5
refine_rows = 5
refine_levels = 1
alphabet = "abcdefghijklmnopqrstuvwxyz"
click_on_select = false
nudge_step = 1
nudge_step_fast = 16
label_font_max_px = 22.0
```

One file works on both platforms: the grid settings are identical, and the
Windows-only trigger keys are parsed and ignored elsewhere rather than rejected.

Unknown keys are rejected rather than ignored, so a typo fails loudly instead of
silently doing nothing. Restart to pick up changes — there is no hot reload yet.

### Triggering on a bare modifier (Windows)

`hotkey` takes a chord (`ctrl+alt+space`) or a **modifier tap**:

```toml
hotkey = "tap:rctrl"        # tap right Ctrl once
hotkey = "doubletap:ctrl"   # tap either Ctrl twice
tap_timeout_ms = 250        # longer than this is a hold, not a tap
double_tap_ms = 350         # max gap between the two taps
```

A bare `hotkey = "ctrl"` is shorthand for `tap:ctrl`. Names: `ctrl`, `alt`,
`shift`, `win` match either side; `lctrl`, `rctrl`, `lalt`, `ralt`, `lshift`,
`rshift`, `lwin`, `rwin` are exact.

A tap means pressed and released within `tap_timeout_ms` with **no other key and
no mouse button or wheel in between**. Holding it, or using it as a modifier,
never triggers.

**`tap:rctrl` is the recommendation.** Almost nothing binds right Ctrl, so a
single tap is unambiguous. `doubletap:ctrl` is the next best if you want either
Ctrl — a single tap of *either* Ctrl misfires often, because releasing Ctrl
without completing a shortcut is something people do constantly.

Only modifiers can be tap triggers. `tap:a` is rejected: distinguishing a tap
from typing would mean swallowing the key, and then you could never type an `a`.

This whole mechanism is a low-level-hook trick and has no Wayland equivalent.
There, use whatever your compositor offers — Hyprland will happily bind
`SUPER, slash` or a double-tap plugin to `my-mouseless toggle`.

Tuning notes:

- `refine_levels = 0` goes straight from the coarse cell to cursor mode: two
  keystrokes, more nudging.
- Keep each grid at or below `alphabet.len()` cells to stay at one keystroke per
  level. 24x14 = 336 needs two characters; 5x5 = 25 needs one.
- Raising `refine_levels` past 1 shrinks labels below readability. Prefer
  nudging.
- `label_font_max_px` caps label text. Only the coarse grid reaches the cap —
  its cells are large enough that the size-from-cell formula saturates, while
  refined cells compute well under it. So this is effectively "level one label
  size", and raising it leaves the refinement alone. Values below 9 or above
  200 are rejected rather than silently clamped.

## Architecture

```
crates/
├─ core/     state machine, geometry, labels — pure, no platform types, unit-tested
├─ os-kit/   Windows: keyboard hook, SendInput, screen geometry, DPI
├─ overlay/  software renderer, plus the Windows layered click-through window
├─ wayland/  layer-shell overlay, keyboard grab, virtual pointer, trigger socket
└─ app/      config, wiring, the decision loop
```

Three threads, each owning exactly one thing:

- **input thread** — the `WH_KEYBOARD_LL` hook and its message pump (Windows), or
  the Wayland connection and its event loop (Linux)
- **trigger thread** — the hotkey, inside the hook (Windows), or the command
  socket (Linux)
- **main thread** — owns the engine, and is the only place decisions are made

`core` deliberately has zero dependencies and no platform types, and `app`'s
decision loop is written once against a small platform façade. That is what lets
the whole of the interesting behaviour be tested without a display, and what
keeps one platform's constraints from quietly becoming the other's behaviour.

### The rule that shapes the Windows backend

`keyboard_proc` runs on the OS input path, ahead of the foreground application.
Exceed `LowLevelHooksTimeout` (300 ms) and Windows silently discards the hook —
the tool goes dead with no error and no log line. So the callback only ever
reads two atomics, decides swallow-or-pass, and `try_send`s a `Copy` struct on a
bounded channel. No allocation, no locks, no blocking, no logging. If the
channel is full the event is dropped, because stalling the input path would
freeze typing machine-wide.

Two consequences worth knowing before changing that file:

- **Modifier keys always pass through**, even while capturing. Swallowing a
  `Ctrl` key-up would leave the foreground application believing `Ctrl` is still
  held — a stuck modifier is worse than the keystroke we were hiding.
- **Injected events always pass through**, or our own `SendInput` clicks would
  feed back into the hook.
- **A tap trigger never swallows its modifier.** Swallowing `Ctrl` would break
  every shortcut on the machine, so in tap mode the foreground application still
  sees a harmless bare `Ctrl` press when the grid opens.

Tap mode also installs a second, passive `WH_MOUSE_LL` hook. It swallows
nothing; it exists solely so `Ctrl`+click cannot be mistaken for a `Ctrl` tap.
Plain mouse movement is ignored, so cursor drift never cancels a tap.

## Wayland notes

Wayland is not Windows-with-different-names here, and three of its rules changed
the design rather than the spelling.

**A client cannot see keys that are not for it.** There is no hook to install, so
the trigger is a compositor binding that runs `my-mouseless toggle`. Once the
grid is up, the overlay's layer surface takes an exclusive keyboard grab, and
every keystroke after that arrives normally — the application underneath sees
nothing, which is exactly what the Windows hook works so hard to fake.

**A surface cannot span monitors.** One layer surface per output, each drawing
its own slice of one shared layout. The renderer already took an origin per
canvas, so this cost nothing; the engine still thinks in one desktop-wide
coordinate space.

**An exclusive keyboard grab takes the pointer with it.** On Hyprland, pointer
events go to a surface holding an exclusive grab whatever its input region says,
and a held button is dropped when that focus moves. Two things follow:

- Clicks work because the engine already hides the overlay *before* clicking, and
  the platform layer re-states the cursor position immediately before the press
  so the compositor has recomputed what is under the cursor.
- A drag cannot be held across the keystrokes that steer the cursor, because
  holding the keyboard is what loses the button. So `v` records an anchor, and
  the press-travel-release is performed as one gesture when the drag ends,
  stepped rather than jumped so applications that grow a selection per motion
  event get motion events to grow it with. The selection appears at the end
  instead of following along.

On-demand keyboard focus was tried instead and reverted: it leaves the pointer
alone, but with focus-follows-mouse the first cursor move hands the keyboard to
whatever window it lands on, and the grid then types its labels into that
window.

Fractional scaling is handled by rendering at the output's integer buffer scale
and letting the compositor scale down; labels and the crosshair grow with it,
rather than shrinking to half size on a HiDPI screen.

## Known gaps

- Config is read once at startup.
- No tray icon — it runs in a console window and quits with `Ctrl+C`, or with
  `my-mouseless quit` on Linux.
- No scroll wheel, and no double click, so double-click-to-select-a-word does
  not work.
- `click_on_select = true` disables cursor mode, and with it dragging and the
  non-left buttons.
- Windows: monitor hot-plug and resolution changes do not resize the overlay;
  restart. Wayland picks up layout changes and cancels any selection in progress.
- Windows: the keyboard layout is read from the hook thread, so a foreground
  application using a different layout can disagree about which character a key
  produces.
- Wayland: a drag does not highlight while you travel, per the note above.
- Wayland: only tested on Hyprland. A compositor without `wlr-layer-shell` or
  `wlr-virtual-pointer` is refused at startup with a message naming what is
  missing; one that grants keyboard focus differently may behave differently
  again.
