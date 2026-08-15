//! The grid-selection state machine.
//!
//! This is the whole behavioural heart of the tool, and it is deliberately
//! pure: inputs in, actions out, no I/O and no platform types. Everything the
//! user can observe about how selection *feels* is decided here and can be
//! tested without a screen.

use crate::config::GridConfig;
use crate::geom::{Point, Rect};
use crate::key::{Key, KeyPress, Mods};
use crate::label;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabeledCell {
    pub label: String,
    pub rect: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Side effects for the host to perform, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Draw these cells. `typed` is the prefix already entered, so the
    /// renderer can dim the consumed characters.
    ShowCells { cells: Vec<LabeledCell>, typed: String },
    /// Draw the cursor-mode crosshair and key hints at this point.
    ///
    /// Cursor mode swallows the keyboard, so it must be visible on screen —
    /// silently eating keystrokes with nothing to show for it is the worst
    /// failure this tool can have.
    ShowCursorHint { pos: Point, dragging: bool },
    Hide,
    MoveCursor(Point),
    Click(Button),
    /// Press and hold, for dragging. Always eventually paired with `MouseUp`.
    MouseDown(Button),
    MouseUp(Button),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Enter grid mode over `region`, normally the whole virtual screen.
    Activate(Rect),
    Key(KeyPress),
    /// Leave grid mode without selecting (hotkey pressed again, focus lost).
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Selecting {
        cells: Vec<LabeledCell>,
        typed: String,
        depth: u32,
    },
    /// The grid is done; the cursor is placed and awaiting a nudge or a click.
    Cursor {
        pos: Point,
    },
}

#[derive(Debug, Clone)]
pub struct Engine {
    cfg: GridConfig,
    state: State,
    /// Region the current activation covers, used to keep nudges on screen.
    region: Rect,
    /// Button held for a drag. Deliberately outside `State`, because a drag
    /// survives re-opening the grid — that is what makes it possible to select
    /// text longer than a nudge can comfortably cross.
    drag: Option<Button>,
}

impl Engine {
    pub fn new(cfg: GridConfig) -> Self {
        Self {
            cfg,
            state: State::Idle,
            region: Rect::new(0, 0, 0, 0),
            drag: None,
        }
    }

    /// The button currently held for a drag, if any.
    pub fn dragging(&self) -> Option<Button> {
        self.drag
    }

    pub fn config(&self) -> &GridConfig {
        &self.cfg
    }

    /// Replace the configuration. Any in-progress selection is abandoned,
    /// because its cells were built from the old grid dimensions.
    pub fn set_config(&mut self, cfg: GridConfig) -> Vec<Action> {
        self.cfg = cfg;
        self.cancel()
    }

    /// Whether keystrokes should be swallowed rather than reaching the
    /// foreground application. The hook thread mirrors this into an atomic.
    pub fn is_capturing(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    pub fn handle(&mut self, input: Input) -> Vec<Action> {
        match input {
            Input::Activate(region) => self.activate(region),
            Input::Cancel => self.cancel(),
            Input::Key(press) => self.on_key(press),
        }
    }

    fn activate(&mut self, region: Rect) -> Vec<Action> {
        self.region = region;
        let cells = self.build_cells(region, self.cfg.coarse_cols, self.cfg.coarse_rows);
        if cells.is_empty() {
            self.state = State::Idle;
            return Vec::new();
        }
        let action = Action::ShowCells {
            cells: cells.clone(),
            typed: String::new(),
        };
        self.state = State::Selecting {
            cells,
            typed: String::new(),
            depth: 0,
        };
        vec![action]
    }

    /// Leave every mode, releasing a held drag button on the way out.
    ///
    /// A drag cannot be "cancelled" in any real sense: the button is already
    /// down, so whatever it selected is selected. Releasing is the only safe
    /// exit — leaving it held would wedge the mouse for the whole session.
    fn cancel(&mut self) -> Vec<Action> {
        if matches!(self.state, State::Idle) && self.drag.is_none() {
            return Vec::new();
        }
        self.state = State::Idle;
        let mut actions = vec![Action::Hide];
        if let Some(button) = self.drag.take() {
            actions.push(Action::MouseUp(button));
        }
        actions
    }

    fn on_key(&mut self, press: KeyPress) -> Vec<Action> {
        // Ctrl/Alt/Win combinations are never ours; let the user keep using
        // them to reach the system while we hold the keyboard.
        let system_chord = press.mods.contains(Mods::CTRL)
            || press.mods.contains(Mods::ALT)
            || press.mods.contains(Mods::WIN);

        match self.state {
            State::Idle => Vec::new(),
            State::Cursor { pos } => self.on_cursor_key(press, pos, system_chord),
            State::Selecting { .. } => match press.key {
                Key::Escape => self.cancel(),
                Key::Backspace => self.backspace(),
                Key::Char(c) if !system_chord => self.push_char(c),
                _ => Vec::new(),
            },
        }
    }

    /// Cursor mode: nudge with arrows or hjkl, commit with a click key.
    fn on_cursor_key(&mut self, press: KeyPress, pos: Point, system_chord: bool) -> Vec<Action> {
        if system_chord {
            return Vec::new();
        }
        match press.key {
            Key::Escape => self.cancel(),
            // While dragging, the commit keys release the button instead of
            // starting a fresh click.
            Key::Space | Key::Enter if self.drag.is_some() => self.cancel(),
            Key::Space | Key::Enter => self.commit(Button::Left),
            Key::Char('v') => self.toggle_drag(pos),
            Key::Char('g') => self.activate(self.region),
            // A click of another button mid-drag would be ambiguous; ignore it.
            Key::Char('r') if self.drag.is_none() => self.commit(Button::Right),
            Key::Char('m') if self.drag.is_none() => self.commit(Button::Middle),
            key => {
                let Some((dx, dy)) = nudge_direction(key) else {
                    return Vec::new();
                };
                let step = if press.mods.contains(Mods::SHIFT) {
                    self.cfg.nudge_step_fast
                } else {
                    self.cfg.nudge_step
                };
                let moved = self
                    .region
                    .clamp(Point::new(pos.x + dx * step, pos.y + dy * step));
                if moved == pos {
                    return Vec::new();
                }
                self.state = State::Cursor { pos: moved };
                vec![Action::MoveCursor(moved), self.hint(moved)]
            }
        }
    }

    /// Start a drag at `pos`, or finish one that is already running.
    fn toggle_drag(&mut self, pos: Point) -> Vec<Action> {
        match self.drag.take() {
            Some(button) => {
                self.state = State::Idle;
                vec![Action::Hide, Action::MouseUp(button)]
            }
            None => {
                self.drag = Some(Button::Left);
                // Stay in cursor mode: the user still has to travel to the end
                // of the selection, by nudging or by re-opening the grid.
                vec![Action::MouseDown(Button::Left), self.hint(pos)]
            }
        }
    }

    fn hint(&self, pos: Point) -> Action {
        Action::ShowCursorHint {
            pos,
            dragging: self.drag.is_some(),
        }
    }

    fn commit(&mut self, button: Button) -> Vec<Action> {
        self.state = State::Idle;
        // Hide before clicking so the overlay is never mistaken for the thing
        // that was clicked.
        vec![Action::Hide, Action::Click(button)]
    }

    fn backspace(&mut self) -> Vec<Action> {
        let State::Selecting { cells, typed, .. } = &mut self.state else {
            return Vec::new();
        };
        if typed.pop().is_none() {
            return Vec::new();
        }
        let typed = typed.clone();
        let visible = filter_by_prefix(cells, &typed);
        vec![Action::ShowCells {
            cells: visible,
            typed,
        }]
    }

    fn push_char(&mut self, c: char) -> Vec<Action> {
        let State::Selecting { cells, typed, depth } = &mut self.state else {
            return Vec::new();
        };

        let mut candidate = typed.clone();
        candidate.push(c);

        let matches = filter_by_prefix(cells, &candidate);
        if matches.is_empty() {
            // Dead keystroke. Swallowed, but nothing changes on screen — the
            // alternative (silently cancelling) loses work the user has done.
            return Vec::new();
        }

        // Labels are fixed-width and unique, so a single match whose label
        // equals the typed string is unambiguously a completed selection.
        if matches.len() == 1 && matches[0].label == candidate {
            let chosen = matches[0].rect;
            let depth = *depth;
            return self.select(chosen, depth);
        }

        *typed = candidate.clone();
        vec![Action::ShowCells {
            cells: matches,
            typed: candidate,
        }]
    }

    /// A cell was fully typed: either refine into it, or commit the cursor.
    fn select(&mut self, rect: Rect, depth: u32) -> Vec<Action> {
        if depth < self.cfg.refine_levels {
            let cells = self.build_cells(rect, self.cfg.refine_cols, self.cfg.refine_rows);
            // A cell can get too small to subdivide meaningfully; committing
            // is a better answer than showing an empty overlay.
            if !cells.is_empty() {
                let action = Action::ShowCells {
                    cells: cells.clone(),
                    typed: String::new(),
                };
                self.state = State::Selecting {
                    cells,
                    typed: String::new(),
                    depth: depth + 1,
                };
                return vec![action];
            }
        }

        let target = rect.center();
        // Auto-click is skipped mid-drag: the button is already down, so a
        // click here would end the selection at the wrong moment.
        if self.cfg.click_on_select && self.drag.is_none() {
            self.state = State::Idle;
            return vec![
                Action::Hide,
                Action::MoveCursor(target),
                Action::Click(Button::Left),
            ];
        }

        // Hand over to cursor mode: place the cursor, swap the grid for the
        // crosshair, and wait for the user to fine-tune and choose a button.
        self.state = State::Cursor { pos: target };
        vec![Action::MoveCursor(target), self.hint(target)]
    }

    fn build_cells(&self, region: Rect, cols: u32, rows: u32) -> Vec<LabeledCell> {
        let rects = region.subdivide(cols, rows);
        let labels = label::generate(&self.cfg.alphabet, rects.len());
        // `generate` returns nothing for a degenerate alphabet; zipping would
        // silently produce an empty grid, which `activate`/`select` handle.
        labels
            .into_iter()
            .zip(rects)
            .map(|(label, rect)| LabeledCell { label, rect })
            .collect()
    }
}

/// Unit direction for a nudge key, or `None` if the key does not move.
///
/// Arrows and hjkl both work, so neither vim habits nor their absence are
/// punished.
fn nudge_direction(key: Key) -> Option<(i32, i32)> {
    Some(match key {
        Key::Left | Key::Char('h') => (-1, 0),
        Key::Down | Key::Char('j') => (0, 1),
        Key::Up | Key::Char('k') => (0, -1),
        Key::Right | Key::Char('l') => (1, 0),
        _ => return None,
    })
}

fn filter_by_prefix(cells: &[LabeledCell], prefix: &str) -> Vec<LabeledCell> {
    if prefix.is_empty() {
        return cells.to_vec();
    }
    cells
        .iter()
        .filter(|c| c.label.starts_with(prefix))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

    fn engine(refine_levels: u32) -> Engine {
        Engine::new(GridConfig {
            coarse_cols: 4,
            coarse_rows: 4,
            refine_cols: 2,
            refine_rows: 2,
            refine_levels,
            alphabet: ('a'..='z').collect(),
            click_on_select: true,
            ..Default::default()
        })
    }

    fn press(c: char) -> Input {
        Input::Key(KeyPress::plain(Key::Char(c)))
    }

    fn shown(actions: &[Action]) -> Option<(&Vec<LabeledCell>, &String)> {
        actions.iter().find_map(|a| match a {
            Action::ShowCells { cells, typed } => Some((cells, typed)),
            _ => None,
        })
    }

    #[test]
    fn activation_shows_the_full_coarse_grid() {
        let mut e = engine(0);
        let actions = e.handle(Input::Activate(SCREEN));
        let (cells, typed) = shown(&actions).expect("should show cells");
        assert_eq!(cells.len(), 16);
        assert!(typed.is_empty());
        assert!(e.is_capturing());
    }

    #[test]
    fn typing_a_full_label_commits_cursor_and_click() {
        let mut e = engine(0);
        let actions = e.handle(Input::Activate(SCREEN));
        let (cells, _) = shown(&actions).unwrap();
        // 16 cells over 26 symbols => single-character labels.
        let target = cells[5].clone();

        let actions = e.handle(press(target.label.chars().next().unwrap()));
        assert_eq!(
            actions,
            vec![
                Action::Hide,
                Action::MoveCursor(target.rect.center()),
                Action::Click(Button::Left),
            ]
        );
        assert!(!e.is_capturing(), "engine returns to idle after committing");
    }

    #[test]
    fn refinement_subdivides_before_committing() {
        let mut e = engine(1);
        let actions = e.handle(Input::Activate(SCREEN));
        let coarse = shown(&actions).unwrap().0.clone();
        let target = coarse[0].clone();

        // First selection refines rather than moving the cursor.
        let actions = e.handle(press(target.label.chars().next().unwrap()));
        let (fine, typed) = shown(&actions).expect("refinement should show cells");
        assert_eq!(fine.len(), 4);
        assert!(typed.is_empty(), "typed prefix resets between levels");
        assert!(e.is_capturing());

        // Every refined cell lies inside the cell we picked.
        for cell in fine {
            assert!(cell.rect.x >= target.rect.x && cell.rect.right() <= target.rect.right());
            assert!(cell.rect.y >= target.rect.y && cell.rect.bottom() <= target.rect.bottom());
        }

        // Second selection commits.
        let fine = shown(&actions).unwrap().0.clone();
        let actions = e.handle(press(fine[3].label.chars().next().unwrap()));
        assert!(actions.contains(&Action::MoveCursor(fine[3].rect.center())));
        assert!(!e.is_capturing());
    }

    #[test]
    fn multi_char_labels_narrow_progressively() {
        let mut e = Engine::new(GridConfig {
            coarse_cols: 10,
            coarse_rows: 10,
            refine_levels: 0,
            alphabet: ('a'..='e').collect(), // 5 symbols, 100 cells => 3 chars
            ..Default::default()
        });
        let actions = e.handle(Input::Activate(SCREEN));
        let cells = shown(&actions).unwrap().0.clone();
        assert_eq!(cells[0].label.chars().count(), 3);

        let target = cells[42].clone();
        let mut chars = target.label.chars();

        let actions = e.handle(press(chars.next().unwrap()));
        let (narrowed, typed) = shown(&actions).unwrap();
        assert_eq!(typed.chars().count(), 1);
        assert!(narrowed.len() < cells.len() && narrowed.len() > 1);

        let actions = e.handle(press(chars.next().unwrap()));
        let (narrowed, _) = shown(&actions).unwrap();
        assert!(narrowed.len() > 1);

        let actions = e.handle(press(chars.next().unwrap()));
        assert!(actions.contains(&Action::MoveCursor(target.rect.center())));
    }

    #[test]
    fn unmatched_keystroke_is_ignored_not_destructive() {
        let mut e = engine(0);
        e.handle(Input::Activate(SCREEN));
        // 16 cells => labels 'a'..'p'. 'z' matches nothing.
        assert_eq!(e.handle(press('z')), Vec::new());
        assert!(e.is_capturing(), "a dead key must not cancel the selection");
    }

    #[test]
    fn backspace_widens_the_match_set() {
        let mut e = Engine::new(GridConfig {
            coarse_cols: 10,
            coarse_rows: 10,
            refine_levels: 0,
            alphabet: ('a'..='e').collect(),
            ..Default::default()
        });
        let actions = e.handle(Input::Activate(SCREEN));
        let all = shown(&actions).unwrap().0.len();

        let actions = e.handle(press('c'));
        let narrowed = shown(&actions).unwrap().0.len();
        assert!(narrowed < all);

        let actions = e.handle(Input::Key(KeyPress::plain(Key::Backspace)));
        let (widened, typed) = shown(&actions).unwrap();
        assert_eq!(widened.len(), all);
        assert!(typed.is_empty());
    }

    #[test]
    fn backspace_on_empty_prefix_does_nothing() {
        let mut e = engine(0);
        e.handle(Input::Activate(SCREEN));
        assert_eq!(e.handle(Input::Key(KeyPress::plain(Key::Backspace))), vec![]);
        assert!(e.is_capturing());
    }

    #[test]
    fn escape_hides_and_idles() {
        let mut e = engine(0);
        e.handle(Input::Activate(SCREEN));
        assert_eq!(
            e.handle(Input::Key(KeyPress::plain(Key::Escape))),
            vec![Action::Hide]
        );
        assert!(!e.is_capturing());
    }

    #[test]
    fn keys_are_ignored_while_idle() {
        let mut e = engine(0);
        assert_eq!(e.handle(press('a')), Vec::new());
        assert_eq!(e.handle(Input::Cancel), Vec::new());
        assert!(!e.is_capturing());
    }

    #[test]
    fn modified_chars_do_not_count_as_label_input() {
        let mut e = engine(0);
        e.handle(Input::Activate(SCREEN));
        let ctrl_a = Input::Key(KeyPress::new(Key::Char('a'), Mods::CTRL));
        assert_eq!(e.handle(ctrl_a), Vec::new());
        assert!(e.is_capturing());
    }

    /// Selection hands over to cursor mode instead of clicking.
    fn cursor_engine() -> Engine {
        Engine::new(GridConfig {
            coarse_cols: 4,
            coarse_rows: 4,
            refine_levels: 0,
            click_on_select: false,
            nudge_step: 1,
            nudge_step_fast: 16,
            ..Default::default()
        })
    }

    /// Drive `cursor_engine` through a selection; returns the resting cursor.
    fn enter_cursor_mode(e: &mut Engine) -> Point {
        let actions = e.handle(Input::Activate(SCREEN));
        let label = shown(&actions).unwrap().0[0].label.clone();
        let actions = e.handle(press(label.chars().next().unwrap()));
        match actions.as_slice() {
            [Action::MoveCursor(p), Action::ShowCursorHint { pos, dragging }] => {
                assert_eq!(p, pos, "cursor and hint must agree");
                assert!(!dragging);
                *p
            }
            other => panic!("expected handover to cursor mode, got {other:?}"),
        }
    }

    #[test]
    fn selection_hands_over_to_cursor_mode_when_autoclick_is_off() {
        let mut e = cursor_engine();
        let pos = enter_cursor_mode(&mut e);
        // First of a 4x4 grid over 1920x1080 => cell 0,0..480x270.
        assert_eq!(pos, Point::new(240, 135));
        assert!(e.is_capturing(), "cursor mode still owns the keyboard");
    }

    #[test]
    fn cursor_mode_nudges_with_arrows_and_hjkl() {
        let mut e = cursor_engine();
        let start = enter_cursor_mode(&mut e);

        let actions = e.handle(Input::Key(KeyPress::plain(Key::Right)));
        assert_eq!(
            actions,
            vec![
                Action::MoveCursor(Point::new(start.x + 1, start.y)),
                Action::ShowCursorHint { pos: Point::new(start.x + 1, start.y), dragging: false },
            ]
        );

        // hjkl mirrors the arrows.
        e.handle(press('j'));
        let actions = e.handle(press('h'));
        assert_eq!(
            actions[0],
            Action::MoveCursor(Point::new(start.x, start.y + 1))
        );
    }

    #[test]
    fn shift_uses_the_fast_step() {
        let mut e = cursor_engine();
        let start = enter_cursor_mode(&mut e);
        let actions = e.handle(Input::Key(KeyPress::new(Key::Down, Mods::SHIFT)));
        assert_eq!(
            actions[0],
            Action::MoveCursor(Point::new(start.x, start.y + 16))
        );
    }

    #[test]
    fn nudging_is_clamped_to_the_activation_region() {
        let mut e = cursor_engine();
        enter_cursor_mode(&mut e);
        // Walk hard into the left edge; it must stop, not run negative.
        for _ in 0..400 {
            e.handle(Input::Key(KeyPress::new(Key::Left, Mods::SHIFT)));
        }
        let actions = e.handle(Input::Key(KeyPress::plain(Key::Left)));
        assert!(
            actions.is_empty(),
            "a nudge that cannot move should emit nothing"
        );
    }

    #[test]
    fn cursor_mode_clicks_and_exits() {
        for (key, button) in [
            (Input::Key(KeyPress::plain(Key::Space)), Button::Left),
            (Input::Key(KeyPress::plain(Key::Enter)), Button::Left),
            (press('r'), Button::Right),
            (press('m'), Button::Middle),
        ] {
            let mut e = cursor_engine();
            enter_cursor_mode(&mut e);
            assert_eq!(
                e.handle(key),
                vec![Action::Hide, Action::Click(button)],
                "wrong result for {button:?}"
            );
            assert!(!e.is_capturing(), "clicking releases the keyboard");
        }
    }

    /// Enter cursor mode and start a drag; returns the anchor point.
    fn start_drag(e: &mut Engine) -> Point {
        let pos = enter_cursor_mode(e);
        let actions = e.handle(press('v'));
        assert_eq!(
            actions,
            vec![
                Action::MouseDown(Button::Left),
                Action::ShowCursorHint {
                    pos,
                    dragging: true
                },
            ]
        );
        assert_eq!(e.dragging(), Some(Button::Left));
        pos
    }

    #[test]
    fn drag_holds_the_button_and_stays_in_cursor_mode() {
        let mut e = cursor_engine();
        start_drag(&mut e);
        assert!(e.is_capturing(), "still driving the cursor while dragging");
    }

    #[test]
    fn nudging_while_dragging_reports_drag_state_to_the_renderer() {
        let mut e = cursor_engine();
        let start = start_drag(&mut e);
        let actions = e.handle(Input::Key(KeyPress::plain(Key::Right)));
        assert_eq!(
            actions[1],
            Action::ShowCursorHint {
                pos: Point::new(start.x + 1, start.y),
                dragging: true
            }
        );
    }

    #[test]
    fn drag_survives_reopening_the_grid() {
        // This is the whole point: cross a long selection by picking the end
        // point from the grid rather than nudging there one pixel at a time.
        let mut e = cursor_engine();
        start_drag(&mut e);

        let actions = e.handle(press('g'));
        assert!(
            shown(&actions).is_some(),
            "'g' should bring the grid back up"
        );
        assert_eq!(e.dragging(), Some(Button::Left), "button stays held");

        // Completing the new selection returns to cursor mode, still dragging.
        let label = shown(&actions).unwrap().0[3].label.clone();
        let actions = e.handle(press(label.chars().next().unwrap()));
        assert!(matches!(
            actions.as_slice(),
            [Action::MoveCursor(_), Action::ShowCursorHint { dragging: true, .. }]
        ));
        assert_eq!(e.dragging(), Some(Button::Left));
    }

    #[test]
    fn second_v_releases_the_button() {
        let mut e = cursor_engine();
        start_drag(&mut e);
        assert_eq!(
            e.handle(press('v')),
            vec![Action::Hide, Action::MouseUp(Button::Left)]
        );
        assert_eq!(e.dragging(), None);
        assert!(!e.is_capturing());
    }

    #[test]
    fn space_finishes_a_drag_instead_of_clicking_again() {
        let mut e = cursor_engine();
        start_drag(&mut e);
        let actions = e.handle(Input::Key(KeyPress::plain(Key::Space)));
        assert_eq!(actions, vec![Action::Hide, Action::MouseUp(Button::Left)]);
        assert!(
            !actions.iter().any(|a| matches!(a, Action::Click(_))),
            "a drag must not end with an extra click"
        );
    }

    /// Every exit path must release a held button. A stuck synthetic button
    /// cannot be cleared by the user, because they never physically pressed it.
    #[test]
    fn every_exit_path_releases_a_held_button() {
        type Exit = Box<dyn Fn(&mut Engine) -> Vec<Action>>;
        let exits: Vec<(&str, Exit)> = vec![
            (
                "escape",
                Box::new(|e: &mut Engine| e.handle(Input::Key(KeyPress::plain(Key::Escape)))),
            ),
            ("cancel/hotkey", Box::new(|e: &mut Engine| e.handle(Input::Cancel))),
            (
                "reconfigure",
                Box::new(|e: &mut Engine| e.set_config(GridConfig::default())),
            ),
            ("toggle v", Box::new(|e: &mut Engine| e.handle(press('v')))),
            (
                "space",
                Box::new(|e: &mut Engine| e.handle(Input::Key(KeyPress::plain(Key::Space)))),
            ),
        ];

        for (name, exit) in exits {
            let mut e = cursor_engine();
            start_drag(&mut e);
            let actions = exit(&mut e);
            assert!(
                actions.contains(&Action::MouseUp(Button::Left)),
                "exit via {name} left the button held"
            );
            assert_eq!(e.dragging(), None, "exit via {name} left drag state set");
        }
    }

    #[test]
    fn other_buttons_are_ignored_mid_drag() {
        for key in ['r', 'm'] {
            let mut e = cursor_engine();
            start_drag(&mut e);
            assert_eq!(e.handle(press(key)), Vec::new(), "{key} should be inert");
            assert_eq!(e.dragging(), Some(Button::Left));
        }
    }

    /// `click_on_select` commits straight from the grid, so cursor mode — and
    /// with it dragging — is never reached. That is a real tradeoff of the
    /// setting, not an oversight.
    #[test]
    fn click_on_select_bypasses_cursor_mode_and_therefore_drag() {
        let mut e = engine(0); // click_on_select: true
        e.handle(Input::Activate(SCREEN));
        let actions = e.handle(press('a'));

        assert!(actions.iter().any(|a| matches!(a, Action::Click(_))));
        assert!(!e.is_capturing());
        assert_eq!(
            e.dragging(),
            None,
            "no cursor mode means no way to start a drag"
        );
    }

    #[test]
    fn hotkey_cancels_from_cursor_mode() {
        // The hook forwards the hotkey as Cancel from any mode, so this must
        // unwind cursor mode as well as an in-progress selection.
        let mut e = cursor_engine();
        enter_cursor_mode(&mut e);
        assert_eq!(e.handle(Input::Cancel), vec![Action::Hide]);
        assert!(!e.is_capturing());
    }

    #[test]
    fn escape_leaves_cursor_mode_without_clicking() {
        let mut e = cursor_engine();
        enter_cursor_mode(&mut e);
        assert_eq!(
            e.handle(Input::Key(KeyPress::plain(Key::Escape))),
            vec![Action::Hide]
        );
        assert!(!e.is_capturing());
    }

    #[test]
    fn cursor_mode_ignores_system_chords() {
        let mut e = cursor_engine();
        enter_cursor_mode(&mut e);
        let ctrl_r = Input::Key(KeyPress::new(Key::Char('r'), Mods::CTRL));
        assert_eq!(e.handle(ctrl_r), Vec::new(), "Ctrl+R must not right-click");
        assert!(e.is_capturing());
    }

    #[test]
    fn autoclick_still_available_when_enabled() {
        let mut e = engine(0); // click_on_select: true
        let actions = e.handle(Input::Activate(SCREEN));
        let label = shown(&actions).unwrap().0[0].label.clone();
        let actions = e.handle(press(label.chars().next().unwrap()));
        assert!(actions.iter().any(|a| matches!(a, Action::Click(_))));
        assert!(!e.is_capturing());
    }

    #[test]
    fn reconfiguring_mid_selection_abandons_it() {
        let mut e = engine(0);
        e.handle(Input::Activate(SCREEN));
        assert_eq!(e.set_config(GridConfig::default()), vec![Action::Hide]);
        assert!(!e.is_capturing());
    }
}
