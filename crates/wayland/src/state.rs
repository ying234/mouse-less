//! The Wayland client: layer-shell overlay, keyboard grab, virtual pointer.
//!
//! All three live on one thread because they share one connection, and a
//! Wayland event queue belongs to whoever dispatches it. That is also the
//! reason cursor movement is handled here rather than from the engine thread:
//! it keeps `Hide` and the click that follows it in a fixed order, so the
//! overlay is never still on screen when the click lands.

use crossbeam_channel::Sender;
use mouseless_core::{Button, LabeledCell, Point, Rect};
use mouseless_overlay::{Canvas, RenderOptions, Renderer};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};

use crate::keymap;
use crate::pointer::VirtualPointer;
use crate::{Error, Event};

/// How long to wait after losing the keyboard before believing it.
///
/// Long enough to cover an unmap and remap in the same breath, short enough
/// that a genuinely stranded overlay does not sit there ignoring the user.
const FOCUS_SETTLE: std::time::Duration = std::time::Duration::from_millis(120);

/// Intermediate positions sent while performing a deferred drag. Enough for an
/// application to see the selection grow; few enough to stay one gesture.
const DRAG_STEPS: i32 = 8;

/// Work for the Wayland thread, in the order the engine produced it.
#[derive(Debug, Clone)]
pub(crate) enum Cmd {
    Show(Frame),
    Hide,
    MoveCursor(Point),
    Click(Button),
    MouseDown(Button),
    MouseUp(Button),
    Quit,
}

/// What a repaint should draw.
#[derive(Debug, Clone)]
pub(crate) enum Frame {
    Cells {
        cells: Vec<LabeledCell>,
        typed: String,
    },
    CursorHint {
        pos: Point,
        dragging: bool,
    },
}

/// One layer surface, covering one output.
///
/// Wayland has no desktop-spanning surface: a layer surface belongs to exactly
/// one output. So the overlay is really N overlays drawing N windows onto one
/// shared layout, which the renderer already supports through its origin
/// parameter.
struct OutputSurface {
    layer: LayerSurface,
    /// The output's place in the layout, in logical pixels.
    logical: Rect,
    /// Device pixels per logical pixel on this output.
    scale: i32,
    /// Size the compositor told us to be, in logical pixels. Until the first
    /// configure arrives there is nothing valid to draw into.
    configured: Option<(u32, u32)>,
}

pub(crate) struct State {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    compositor: CompositorState,
    layer_shell: LayerShell,
    pool: SlotPool,
    renderer: Renderer,
    options: RenderOptions,

    loop_handle: LoopHandle<'static, State>,
    qh: QueueHandle<State>,

    keyboard: Option<wl_keyboard::WlKeyboard>,
    mods: mouseless_core::Mods,
    pointer_manager: ZwlrVirtualPointerManagerV1,
    pointer: Option<VirtualPointer>,

    events: Sender<Event>,
    surfaces: Vec<OutputSurface>,
    frame: Option<Frame>,
    /// Where we last put the cursor, so a button press can re-state it.
    last_pos: Option<Point>,
    /// Whether the overlay currently holds the keyboard.
    keyboard_focus: bool,
    /// Where a drag was anchored, and with which button, once the user has
    /// pressed `v` but not yet chosen the far end.
    drag_anchor: Option<(Point, Button)>,
    /// Bounding box of the whole layout, in logical pixels. The engine works
    /// in these coordinates and so does the virtual pointer.
    pub(crate) bounds: Rect,
    exit: bool,
}

/// Connect, bind globals, and run the event loop until told to quit.
///
/// `ready` receives the layout bounds once they are known, or the error that
/// stopped us; the caller cannot proceed without them.
pub(crate) fn run(
    options: RenderOptions,
    events: Sender<Event>,
    commands: smithay_client_toolkit::reexports::calloop::channel::Channel<Cmd>,
    ready: std::sync::mpsc::SyncSender<Result<Rect, Error>>,
) {
    match setup(options, events) {
        Ok((state, event_loop, conn, queue)) => {
            let _ = ready.send(Ok(state.bounds));
            pump(state, event_loop, conn, queue, commands);
        }
        Err(e) => {
            let _ = ready.send(Err(e));
        }
    }
}

type Setup = (
    State,
    EventLoop<'static, State>,
    Connection,
    smithay_client_toolkit::reexports::client::EventQueue<State>,
);

fn setup(options: RenderOptions, events: Sender<Event>) -> Result<Setup, Error> {
    let conn = Connection::connect_to_env().map_err(|e| Error::Connect(e.to_string()))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| Error::Connect(e.to_string()))?;
    let qh = queue.handle();

    let event_loop: EventLoop<State> =
        EventLoop::try_new().map_err(|e| Error::Loop(e.to_string()))?;

    let compositor =
        CompositorState::bind(&globals, &qh).map_err(|_| Error::MissingGlobal("wl_compositor"))?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|_| Error::MissingGlobal("zwlr_layer_shell_v1"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| Error::MissingGlobal("wl_shm"))?;
    let pointer_manager = globals
        .bind::<ZwlrVirtualPointerManagerV1, _, _>(&qh, 1..=2, ())
        .map_err(|_| Error::MissingGlobal("zwlr_virtual_pointer_manager_v1"))?;

    // One screen's worth to start with; the pool grows itself when a buffer
    // needs more than it has.
    let pool = SlotPool::new(1920 * 1080 * 4, &shm).map_err(|e| Error::Shm(e.to_string()))?;
    let renderer = Renderer::new(options).map_err(|e| Error::Font(e.to_string()))?;

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        compositor,
        layer_shell,
        pool,
        renderer,
        options,
        loop_handle: event_loop.handle(),
        qh: qh.clone(),
        keyboard: None,
        mods: mouseless_core::Mods::NONE,
        pointer_manager,
        pointer: None,
        events,
        surfaces: Vec::new(),
        frame: None,
        last_pos: None,
        keyboard_focus: false,
        drag_anchor: None,
        bounds: Rect::new(0, 0, 0, 0),
        exit: false,
    };

    // Two rounds: the first brings the globals in, the second the events they
    // emit in reply — output geometry arrives that way, and without it every
    // cell would be placed against a zero-sized screen.
    queue
        .roundtrip(&mut state)
        .map_err(|e| Error::Connect(e.to_string()))?;
    queue
        .roundtrip(&mut state)
        .map_err(|e| Error::Connect(e.to_string()))?;

    state.bounds = layout_bounds(&state.output_state);
    if state.bounds.is_empty() {
        return Err(Error::NoOutputs);
    }

    let seat = state
        .seat_state
        .seats()
        .next()
        .ok_or(Error::MissingGlobal("wl_seat"))?;
    let pointer = state
        .pointer_manager
        .create_virtual_pointer(Some(&seat), &qh, ());
    state.pointer = Some(VirtualPointer::new(pointer));

    Ok((state, event_loop, conn, queue))
}

fn pump(
    mut state: State,
    mut event_loop: EventLoop<'static, State>,
    conn: Connection,
    queue: smithay_client_toolkit::reexports::client::EventQueue<State>,
    commands: smithay_client_toolkit::reexports::calloop::channel::Channel<Cmd>,
) {
    let handle = event_loop.handle();
    if let Err(e) = WaylandSource::new(conn.clone(), queue).insert(handle.clone()) {
        eprintln!("wayland: could not watch the connection: {e}");
        return;
    }

    let inserted = handle.insert_source(commands, |event, _, state| {
        use smithay_client_toolkit::reexports::calloop::channel::Event as ChannelEvent;
        match event {
            ChannelEvent::Msg(cmd) => state.handle(cmd),
            // The engine thread is gone; there is no one left to draw for.
            ChannelEvent::Closed => state.exit = true,
        }
    });
    if let Err(e) = inserted {
        eprintln!("wayland: could not watch for commands: {e}");
        return;
    }

    loop {
        if state.exit {
            break;
        }
        if let Err(e) = event_loop.dispatch(None, &mut state) {
            eprintln!("wayland: event loop stopped: {e}");
            break;
        }
        // Requests queue up in the client until something pushes them out.
        // Cursor movement is a request with no reply, so without this a nudge
        // would sit unsent until the next unrelated event arrived.
        let _ = conn.flush();
    }

    state.hide();
    state.pointer = None;
    let _ = conn.flush();
}

/// Bounding box of every output, in logical pixels.
fn layout_bounds(outputs: &OutputState) -> Rect {
    let rects = outputs
        .outputs()
        .filter_map(|output| outputs.info(&output))
        .filter_map(|info| output_rect(&info));
    bounding_box(rects)
}

/// Smallest rect covering all of `rects`, or an empty rect if there are none.
fn bounding_box(rects: impl Iterator<Item = Rect>) -> Rect {
    let mut bounds: Option<Rect> = None;
    for rect in rects {
        bounds = Some(match bounds {
            None => rect,
            Some(acc) => {
                let (x, y) = (acc.x.min(rect.x), acc.y.min(rect.y));
                Rect::new(
                    x,
                    y,
                    acc.right().max(rect.right()) - x,
                    acc.bottom().max(rect.bottom()) - y,
                )
            }
        });
    }
    bounds.unwrap_or(Rect::new(0, 0, 0, 0))
}

/// Positions to walk through when performing a deferred drag, starting at the
/// step after `anchor` and finishing exactly on `end`.
fn drag_path(anchor: Point, end: Point, steps: i32) -> Vec<Point> {
    let steps = steps.max(1);
    (1..=steps)
        .map(|step| {
            let at = |from: i32, to: i32| from + (to - from) * step / steps;
            Point::new(at(anchor.x, end.x), at(anchor.y, end.y))
        })
        .collect()
}

/// One output's place in the layout, in logical pixels.
///
/// `xdg_output` reports this directly. Falling back to the mode divided by the
/// scale covers a compositor that does not implement it, at the cost of not
/// knowing where the output sits — which is right for the single-monitor case
/// and wrong for any other, so it is a fallback and not the main path.
fn output_rect(info: &OutputInfo) -> Option<Rect> {
    if let (Some((x, y)), Some((w, h))) = (info.logical_position, info.logical_size) {
        return Some(Rect::new(x, y, w, h));
    }
    let mode = info.modes.iter().find(|m| m.current)?;
    let scale = info.scale_factor.max(1);
    Some(Rect::new(
        info.location.0,
        info.location.1,
        mode.dimensions.0 / scale,
        mode.dimensions.1 / scale,
    ))
}

impl State {
    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Show(frame) => self.show(frame),
            Cmd::Hide => self.hide(),
            Cmd::MoveCursor(p) => {
                let bounds = self.bounds;
                self.last_pos = Some(p);
                if let Some(pointer) = &self.pointer {
                    pointer.move_cursor(bounds, p);
                }
            }
            Cmd::Click(button) => {
                self.aim();
                if let Some(pointer) = &mut self.pointer {
                    pointer.click(button);
                }
            }
            // Remember where the drag starts, but do not press yet: see
            // `perform_drag` for why the press cannot happen until the end is
            // known.
            Cmd::MouseDown(button) => match self.last_pos {
                Some(anchor) => self.drag_anchor = Some((anchor, button)),
                None => {
                    if let Some(pointer) = &mut self.pointer {
                        pointer.down(button);
                    }
                }
            },
            Cmd::MouseUp(button) => match self.drag_anchor.take() {
                Some((anchor, held)) => self.perform_drag(anchor, held),
                // No anchor means the press really did happen, which only
                // occurs if we never knew where the cursor was.
                None => {
                    if let Some(pointer) = &mut self.pointer {
                        pointer.up(button);
                    }
                }
            },
            Cmd::Quit => {
                self.hide();
                self.exit = true;
            }
        }
    }

    /// Press at the anchor, travel to where the cursor is now, release.
    ///
    /// On Windows the button is held down for the whole time the user spends
    /// picking the far end of the selection. Here it cannot be: Hyprland gives
    /// every pointer event to a surface holding an exclusive keyboard grab,
    /// and it drops a held button when that focus moves. Since the overlay
    /// must hold the keyboard to read the keys that steer the cursor, a button
    /// held across those keystrokes gets released underneath us.
    ///
    /// So the drag is deferred and then performed in one movement, once both
    /// ends are known. The user sees the same result, only arriving at the end
    /// rather than following along — the selection does not highlight as the
    /// cursor travels.
    fn perform_drag(&mut self, anchor: Point, button: Button) {
        let end = self.last_pos.unwrap_or(anchor);
        let bounds = self.bounds;
        // Nothing below can reach the window while the overlay holds the
        // pointer, and unmapping is what hands it back.
        self.surfaces.clear();
        self.keyboard_focus = false;

        let Some(pointer) = &mut self.pointer else {
            return;
        };
        pointer.move_cursor(bounds, anchor);
        pointer.down(button);
        // Stepped rather than jumped: an application that grows a selection on
        // each motion event needs motion events to grow it with, and one leap
        // from start to finish is a single event.
        for point in drag_path(anchor, end, DRAG_STEPS) {
            pointer.move_cursor(bounds, point);
        }
        pointer.up(button);
        self.last_pos = Some(end);
    }

    /// Re-send the cursor position, immediately before pressing a button.
    ///
    /// Unmapping a surface does not by itself make the compositor work out
    /// which window the pointer is now over — it recomputes that when a
    /// pointer event arrives. Without this, a press sent straight after the
    /// unmap is delivered against the *old* focus and the window under the
    /// cursor never sees it. Requests are processed in order, so a motion to
    /// where the cursor already is forces the recompute to happen first.
    fn aim(&self) {
        let (Some(pos), Some(pointer)) = (self.last_pos, &self.pointer) else {
            return;
        };
        pointer.move_cursor(self.bounds, pos);
    }

    /// Put the overlay on screen, creating its surfaces if they are not up.
    fn show(&mut self, frame: Frame) {
        self.frame = Some(frame);
        if self.surfaces.is_empty() {
            self.create_surfaces();
            // Nothing to paint into yet. The configure that follows will call
            // back here through `LayerShellHandler::configure`.
            return;
        }
        self.draw_all();
    }

    /// Take the overlay off screen and give the keyboard back.
    ///
    /// Destroying the surfaces rather than blanking them is what releases the
    /// exclusive keyboard grab: a mapped layer surface that asks for the
    /// keyboard keeps it, and the user would find their typing going nowhere.
    fn hide(&mut self) {
        self.frame = None;
        self.surfaces.clear();
        self.keyboard_focus = false;
    }

    fn create_surfaces(&mut self) {
        for output in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&output) else {
                continue;
            };
            let Some(logical) = output_rect(&info) else {
                continue;
            };

            let surface = self.compositor.create_surface(&self.qh);

            // Clicks have to reach whatever is underneath: the overlay is
            // something to read, never something to hit. An empty input region
            // is the Wayland spelling of click-through.
            match Region::new(&self.compositor) {
                Ok(region) => surface.set_input_region(Some(region.wl_region())),
                Err(e) => eprintln!("overlay: no input region, clicks may be swallowed: {e}"),
            }

            let layer = self.layer_shell.create_layer_surface(
                &self.qh,
                surface,
                Layer::Overlay,
                Some("my-mouseless"),
                Some(&output),
            );
            layer.set_anchor(Anchor::all());
            // Zero size with all four anchors means "as big as the output".
            layer.set_size(0, 0);
            // -1 opts out of the exclusive-zone layout entirely, so a bar or a
            // dock cannot shrink the overlay away from the area it labels.
            layer.set_exclusive_zone(-1);
            // Exclusive, and it has to be. On-demand would let the pointer
            // through untouched, which would be tidier — but with
            // focus-follows-mouse the first cursor move hands the keyboard to
            // whatever window it lands on, and the grid then types its labels
            // into that window. Holding the keyboard outright is the only
            // arrangement that survives moving the cursor, which is the one
            // thing this program does.
            layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
            // The initial commit carries no buffer: the compositor answers it
            // with a configure telling us what size to be.
            layer.commit();

            self.surfaces.push(OutputSurface {
                layer,
                logical,
                scale: info.scale_factor.max(1),
                configured: None,
            });
        }
    }

    /// Check, a moment from now, whether we really did lose the keyboard.
    ///
    /// A `leave` on its own does not mean much: taking the overlay down and
    /// putting it straight back up — which starting a drag does — produces a
    /// leave immediately followed by an enter, and acting on the leave would
    /// cancel the drag one event after it began. Comparing surfaces is not
    /// enough to tell the two apart, because the destroyed surface's object id
    /// is free for the replacement to reuse. Waiting for the dust to settle
    /// is.
    fn recheck_keyboard_focus(&mut self) {
        let timer = Timer::from_duration(FOCUS_SETTLE);
        let inserted = self
            .loop_handle
            .insert_source(timer, |_, _, state: &mut State| {
                // Still up, still no keyboard: something else took it, and an
                // overlay that cannot be typed at has to go.
                if !state.surfaces.is_empty() && !state.keyboard_focus {
                    let _ = state.events.send(Event::FocusLost);
                }
                TimeoutAction::Drop
            });
        if let Err(e) = inserted {
            eprintln!("overlay: could not watch for keyboard focus: {e}");
        }
    }

    fn draw_all(&mut self) {
        for index in 0..self.surfaces.len() {
            self.draw(index);
        }
    }

    fn draw(&mut self, index: usize) {
        let Some(frame) = self.frame.clone() else {
            return;
        };
        // Disjoint field borrows: the pool, the renderer and one surface.
        let State {
            pool,
            renderer,
            surfaces,
            options,
            ..
        } = self;
        let Some(surface) = surfaces.get(index) else {
            return;
        };
        let Some((lw, lh)) = surface.configured else {
            return;
        };
        if lw == 0 || lh == 0 {
            return;
        }

        let scale = surface.scale.max(1);
        let width = lw as i32 * scale;
        let height = lh as i32 * scale;
        let stride = width * 4;

        let buffer = match pool.create_buffer(width, height, stride, wl_shm::Format::Argb8888) {
            Ok((buffer, data)) => {
                // Wayland's ARGB8888 is a little-endian 32-bit word, so in
                // memory it is B, G, R, A with premultiplied alpha — exactly
                // what the canvas already produces for the Windows path.
                renderer.set_options(RenderOptions {
                    label_font_max_px: options.label_font_max_px * scale as f32,
                    scale: options.scale * scale as f32,
                });
                let mut canvas = Canvas::new(width, height, data);
                paint(renderer, &mut canvas, &frame, surface.logical, scale);
                buffer
            }
            Err(e) => {
                eprintln!("overlay: could not allocate a buffer: {e}");
                return;
            }
        };

        let wl_surface = surface.layer.wl_surface();
        wl_surface.set_buffer_scale(scale);
        wl_surface.damage_buffer(0, 0, width, height);
        if let Err(e) = buffer.attach_to(wl_surface) {
            eprintln!("overlay: could not attach a buffer: {e}");
            return;
        }
        surface.layer.commit();
    }

    fn on_key(&mut self, event: &KeyEvent) {
        let press = mouseless_core::KeyPress::new(keymap::translate(event), self.mods);
        if self.events.send(Event::Key(press)).is_err() {
            self.exit = true;
        }
    }
}

/// Draw one frame onto one output's canvas.
///
/// Cell rects arrive in layout coordinates and leave in that output's device
/// pixels, which is also what makes the labels grow on a HiDPI screen: the
/// size formula reads the cell it is labelling.
fn paint(renderer: &Renderer, canvas: &mut Canvas<'_>, frame: &Frame, logical: Rect, scale: i32) {
    let to_device = |p: Point| Point::new((p.x - logical.x) * scale, (p.y - logical.y) * scale);

    match frame {
        Frame::Cells { cells, typed } => {
            let scaled: Vec<LabeledCell> = cells
                .iter()
                .map(|cell| {
                    let origin = to_device(Point::new(cell.rect.x, cell.rect.y));
                    LabeledCell {
                        label: cell.label.clone(),
                        rect: Rect::new(
                            origin.x,
                            origin.y,
                            cell.rect.w * scale,
                            cell.rect.h * scale,
                        ),
                    }
                })
                .collect();
            renderer.draw(canvas, &scaled, typed, (0, 0));
        }
        Frame::CursorHint { pos, dragging } => {
            // The hint chip is clamped into view by the renderer, which on a
            // second monitor would park a stray copy at its edge. Only the
            // output the cursor is actually on gets one.
            if pos.x < logical.x
                || pos.x >= logical.right()
                || pos.y < logical.y
                || pos.y >= logical.bottom()
            {
                canvas.fill(mouseless_overlay::Rgba::new(0, 0, 0, 0));
                return;
            }
            renderer.draw_cursor_hint(canvas, to_device(*pos), *dragging, (0, 0));
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self.index_of(surface) else {
            return;
        };
        if self.surfaces[index].scale == new_factor.max(1) {
            return;
        }
        self.surfaces[index].scale = new_factor.max(1);
        self.draw(index);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl State {
    fn index_of(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer.wl_surface() == surface)
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.on_outputs_changed();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.on_outputs_changed();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.on_outputs_changed();
    }
}

impl State {
    /// A monitor was plugged, unplugged or moved.
    ///
    /// The layout bounds decide where cells go and how the virtual pointer
    /// maps its coordinates, so a stale copy would send the cursor to the
    /// wrong place entirely. An overlay that is up is torn down: its cells
    /// describe a screen arrangement that no longer exists.
    fn on_outputs_changed(&mut self) {
        let bounds = layout_bounds(&self.output_state);
        if bounds == self.bounds {
            return;
        }
        self.bounds = bounds;
        if !self.surfaces.is_empty() {
            self.hide();
            let _ = self.events.send(Event::LayoutChanged(bounds));
        }
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.surfaces.retain(|s| &s.layer != layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(index) = self.surfaces.iter().position(|s| &s.layer == layer) else {
            return;
        };
        let (w, h) = configure.new_size;
        // A zero here means "pick your own size", which for a full-screen
        // overlay means the output we were told to cover.
        let size = (
            if w == 0 {
                self.surfaces[index].logical.w as u32
            } else {
                w
            },
            if h == 0 {
                self.surfaces[index].logical.h as u32
            } else {
                h
            },
        );
        self.surfaces[index].configured = Some(size);
        self.draw(index);
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability != Capability::Keyboard || self.keyboard.is_some() {
            return;
        }
        // With repeat, because holding a nudge key to travel across the screen
        // is the obvious thing to try and it should work.
        let keyboard = self.seat_state.get_keyboard_with_repeat(
            qh,
            &seat,
            None,
            self.loop_handle.clone(),
            Box::new(|state: &mut State, _kbd, event| state.on_key(&event)),
        );
        match keyboard {
            Ok(keyboard) => self.keyboard = Some(keyboard),
            Err(e) => eprintln!("wayland: no keyboard on this seat: {e}"),
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if self.index_of(surface).is_some() {
            self.keyboard_focus = true;
        }
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        // Losing the grab while the grid is up (a lock screen, a compositor
        // that hands focus elsewhere) would leave an overlay on screen that no
        // longer answers to the keyboard. Treat it as a cancel.
        //
        // Only for a surface we still hold, though. Taking the overlay down
        // also produces a leave, and mid-drag we take it down and put it
        // straight back up — reading that one as lost focus would cancel the
        // drag we just started, one event after starting it.
        if self.index_of(surface).is_some() {
            self.keyboard_focus = false;
            self.recheck_keyboard_focus();
        }
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.on_key(&event);
    }

    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.on_key(&event);
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.mods = keymap::mods(modifiers);
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrVirtualPointerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_registry!(State);

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(State);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounding_box_of_nothing_is_empty() {
        assert!(bounding_box(std::iter::empty()).is_empty());
    }

    #[test]
    fn a_single_output_is_its_own_bounds() {
        let only = Rect::new(0, 0, 1536, 864);
        assert_eq!(bounding_box([only].into_iter()), only);
    }

    #[test]
    fn side_by_side_outputs_are_covered() {
        let bounds =
            bounding_box([Rect::new(0, 0, 1920, 1080), Rect::new(1920, 0, 2560, 1440)].into_iter());
        assert_eq!(bounds, Rect::new(0, 0, 4480, 1440));
    }

    #[test]
    fn an_output_left_of_the_origin_moves_the_origin() {
        // The layout origin is not always (0, 0), and the virtual pointer maps
        // its coordinates against these bounds — getting this wrong sends the
        // cursor to the wrong monitor.
        let bounds = bounding_box(
            [
                Rect::new(0, 0, 1920, 1080),
                Rect::new(-1280, -200, 1280, 1024),
            ]
            .into_iter(),
        );
        assert_eq!(bounds, Rect::new(-1280, -200, 3200, 1280));
    }

    #[test]
    fn a_drag_path_ends_exactly_on_the_target() {
        // Landing one pixel short would select one character too few, every
        // time, which is the sort of thing a user never quite forgives.
        let path = drag_path(Point::new(10, 20), Point::new(133, 77), 8);
        assert_eq!(path.len(), 8);
        assert_eq!(*path.last().unwrap(), Point::new(133, 77));
    }

    #[test]
    fn a_drag_path_moves_in_one_direction() {
        let path = drag_path(Point::new(0, 0), Point::new(100, 50), 5);
        for pair in path.windows(2) {
            assert!(pair[1].x >= pair[0].x && pair[1].y >= pair[0].y, "{pair:?}");
        }
    }

    #[test]
    fn a_zero_length_drag_still_reports_its_position() {
        let same = Point::new(42, 42);
        assert_eq!(drag_path(same, same, 8).last(), Some(&same));
        // A nonsense step count must not produce an empty path: the release
        // would then happen without a single motion to place it.
        assert_eq!(drag_path(same, Point::new(50, 50), 0).len(), 1);
    }
}
