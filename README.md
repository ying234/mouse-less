# my-mouseless

Keyboard-driven cursor control for Windows, in Rust. Press a hotkey, a labelled
grid covers every monitor, type the label, the cursor goes there and clicks.

Status: **working vertical slice.** Grid selection, refinement, cursor mode,
left/right/middle clicking, and drag (text selection) all work end to end. No
tray icon, no settings UI, no scrolling and no double click yet.

## Build and run

```bash
cargo run --release
```

First run writes `%APPDATA%\my-mouseless\config.toml` with the defaults below.

## Using it

Two phases. The grid gets you close, then cursor mode lets you aim and choose a
button.

**Grid** — three keystrokes: two for the coarse cell, one for the refinement.

| Key | Action |
| --- | --- |
| `Ctrl+Alt+Space` | show / hide the grid (configurable; can be a bare `Ctrl` tap) |
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
holds the left button down and `g` re-opens the grid without letting go, so you
can cross a long selection by picking its end point instead of nudging there a
pixel at a time.

1. `Ctrl+Alt+Space`, then the label — cursor lands near the start of the text
2. nudge with `hjkl` to the exact character
3. `v` — left button goes down. The crosshair turns amber and reads `DRAGGING`
4. `g`, then a label — jump to the end of the selection, button still held
5. nudge to the exact end
6. `Space` — button releases, text is selected

For a short selection, skip steps 4–5 and just nudge: `Shift`+`hjkl` moves 16 px
at a time.

Double-click-to-select-a-word is not implemented yet; there is no double click.

**The button is never left held.** Every exit — `Space`, `v`, `Esc`, the hotkey,
a config reload — releases it, and `Ctrl+C` releases it through a console
control handler. This matters because a synthetic button-down that never gets
its matching up cannot be cleared by the user: they never physically pressed it.
There is a test asserting every exit path releases.

A third grid level was tried and removed: it put cells at roughly 6x8 px, where
the labels are too small to read. Precision past two levels comes from nudging
instead, which is also what makes right and middle click reachable.

Set `click_on_select = true` to skip cursor mode and left-click the moment the
grid selection completes. It is faster, but at two levels the cell center can be
~18 px off target, so it misses small controls — and because it never enters
cursor mode, it also disables dragging and right/middle click entirely.

## Configuration

`%APPDATA%\my-mouseless\config.toml`:

```toml
hotkey = "ctrl+alt+space"
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

Unknown keys are rejected rather than ignored, so a typo fails loudly instead of
silently doing nothing. Restart to pick up changes — there is no hot reload yet.

### Triggering on a bare modifier

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
├─ core/     state machine, geometry, labels — pure, no Win32, fully unit-tested
├─ os-kit/   keyboard hook, SendInput, screen geometry, DPI
├─ overlay/  layered click-through window, software renderer
└─ app/      config, wiring, the decision loop
```

Three threads, each owning exactly one thing:

- **hook thread** — owns the `WH_KEYBOARD_LL` hook and its message pump
- **overlay thread** — owns the window and every GDI object
- **main thread** — owns the engine, and is the only place decisions are made

### The rule that shapes everything

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

`core` deliberately has zero dependencies and no platform types. That is what
lets the whole of the interesting behaviour be tested without a display.

## Known gaps

- Monitor hot-plug and resolution changes do not resize the overlay; restart.
- The keyboard layout is read from the hook thread, so a foreground application
  using a different layout can disagree about which character a key produces.
- Config is read once at startup.
- No tray icon — it runs in a console window and quits with `Ctrl+C`.
- No scroll wheel, and no double click, so double-click-to-select-a-word does
  not work.
- `click_on_select = true` disables cursor mode, and with it dragging and the
  non-left buttons.
