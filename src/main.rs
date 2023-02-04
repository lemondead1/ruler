use std::rc::Rc;
use std::error::Error;
use std::f64::consts::PI;
use std::fmt::{Display, Formatter};
use std::time::{Duration, Instant};

use cairo::{Context, Operator, XCBConnection, XCBDrawable, XCBSurface, XCBVisualType};
use glam::{DVec2, IVec2, UVec2};
use x::*;
use xcb::{Connection, render, shape, VoidCookie, x, Xid};
use xcb::x::Mapping::Keyboard;

use crate::geom::{closest_point_below_line_on_circle};

mod geom;

xcb::atoms_struct! {
    #[derive(Debug)]
    struct Atoms {
        wm_protocols => b"WM_PROTOCOLS",
        wm_del_window => b"WM_DELETE_WINDOW",
        motif_wm_hints => b"_MOTIF_WM_HINTS",
        net_wm_state => b"_NET_WM_STATE",
        new_wm_state_skip_pager => b"_NET_WM_STATE_SKIP_PAGER",
        net_wm_state_above => b"_NET_WM_STATE_ABOVE",
        net_wm_state_sticky => b"_NET_WM_STATE_STICKY",
        net_wm_allowed_actions => b"_NET_WM_ALLOWED_ACTIONS",
        new_wm_action_close => b"_NEW_WM_ACTION_CLOSE",
    }
}

const RULER_HALF_WIDTH: f64 = 40.0;
const TITLE: &str = "Ruler";
const INITIAL_LENGTH: f64 = 400.0;
const CONTROL_RADIUS: f64 = 20.0;
const MIN_LENGTH: f64 = 200.0;

#[derive(Debug, Copy, Clone)]
struct VersionMismatchError {
    client_major_version: u32,
    client_minor_version: u32,
    server_major_version: u32,
    server_minor_version: u32,
    extension_name: &'static str,
}

impl Display for VersionMismatchError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Versions of extension '{}' do not match. Server: (major {}, minor {}) Client: (major {}, minor {})", self.extension_name, self.server_major_version, self.server_minor_version, self.client_major_version, self.client_minor_version)
    }
}

impl Error for VersionMismatchError {}

#[derive(Debug, Copy, Clone)]
struct WindowGeometry {
    x: i16,
    y: i16,
    w: u16,
    h: u16,
}

impl WindowGeometry {
    fn pos(&self) -> IVec2 {
        IVec2::new(self.x as i32, self.y as i32)
    }

    fn size(&self) -> UVec2 {
        UVec2::new(self.w as u32, self.h as u32)
    }
}

#[derive(Copy, Clone)]
enum Dragging {
    From,
    To,
    None,
}

struct XCBObjects {
    conn: Connection,
    atoms: Atoms,
    screen: ScreenBuf,
    window: Window,
    colormap: Colormap,
    depth: DepthBuf,
    gcontext: Gcontext,
    visual_type: Visualtype,
}

fn check_versions(client_major: u32, client_minor: u32, server_major: u32, server_minor: u32, extension: &'static str) -> Result<(), Box<VersionMismatchError>> {
    if server_major != client_major || server_major != client_major {
        Err(Box::new(VersionMismatchError {
            client_major_version: client_major,
            client_minor_version: client_minor,
            server_major_version: server_major,
            server_minor_version: server_minor,
            extension_name: extension,
        }))
    } else {
        Ok(())
    }
}

impl XCBObjects {
    fn setup(width: u16, height: u16) -> Result<XCBObjects, Box<dyn Error>> {
        let (conn, screen_num) = Connection::connect(None)?;

        let cookie = conn.send_request(&render::QueryVersion {
            client_major_version: render::MAJOR_VERSION,
            client_minor_version: render::MINOR_VERSION,
        });
        let reply = conn.wait_for_reply(cookie)?;
        check_versions(render::MAJOR_VERSION, render::MINOR_VERSION,
                       reply.major_version(), reply.minor_version(), render::XNAME)?;

        let cookie = conn.send_request(&shape::QueryVersion {});
        let reply = conn.wait_for_reply(cookie)?;
        check_versions(shape::MAJOR_VERSION, shape::MINOR_VERSION,
                       reply.major_version() as u32, reply.minor_version() as u32, render::XNAME)?;


        let xcb = {
            let atoms = Atoms::intern_all(&conn)?;
            let screen = conn.get_setup().roots().nth(screen_num as usize).unwrap();
            let screen_buf = screen.to_owned();
            let colormap: Colormap = conn.generate_id();
            let depth = screen.allowed_depths().find(|d| d.depth() == 32).unwrap().to_owned();
            let visual_type = depth.visuals().iter().find(|v| v.class() == VisualClass::TrueColor).unwrap().clone();
            let window: Window = conn.generate_id();
            let gcontext = conn.generate_id();

            XCBObjects { conn, atoms, screen: screen_buf, depth, visual_type, window, gcontext, colormap }
        };

        let root = xcb.screen.root();

        xcb.conn.send_and_check_request(&CreateColormap {
            alloc: ColormapAlloc::None,
            mid: xcb.colormap,
            window: root,
            visual: xcb.visual_type.visual_id(),
        })?;

        xcb.conn.send_and_check_request(&CreateWindow {
            depth: xcb.depth.depth(),
            wid: xcb.window,
            parent: root,
            x: 0,
            y: 0,
            width,
            height,
            border_width: 0,
            class: WindowClass::InputOutput,
            visual: xcb.visual_type.visual_id(),
            value_list: &[
                Cw::BorderPixel(0x00000000),
                Cw::WinGravity(Gravity::NorthWest),
                Cw::EventMask(EventMask::EXPOSURE | EventMask::KEY_PRESS | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::STRUCTURE_NOTIFY),
                Cw::Colormap(xcb.colormap)
            ],
        })?;

        xcb.conn.send_and_check_request(&ChangeProperty {
            mode: PropMode::Replace,
            window: xcb.window,
            property: xcb.atoms.motif_wm_hints,
            r#type: ATOM_INTEGER,
            data: &[2u32, 0u32, 0u32, 0u32, 0u32],
        })?;

        xcb.conn.send_and_check_request(&ChangeProperty {
            mode: PropMode::Replace,
            window: xcb.window,
            property: ATOM_WM_NAME,
            r#type: ATOM_STRING,
            data: TITLE.as_bytes(),
        })?;

        xcb.conn.send_and_check_request(&ChangeProperty {
            mode: PropMode::Replace,
            window: xcb.window,
            property: xcb.atoms.wm_protocols,
            r#type: ATOM_ATOM,
            data: &[xcb.atoms.wm_del_window],
        })?;

        xcb.conn.send_and_check_request(&ChangeProperty {
            mode: PropMode::Replace,
            window: xcb.window,
            property: xcb.atoms.net_wm_state,
            r#type: ATOM_ATOM,
            data: &[xcb.atoms.net_wm_state_above, xcb.atoms.new_wm_state_skip_pager],
        })?;

        xcb.conn.send_and_check_request(&ChangeProperty {
            mode: PropMode::Replace,
            window: xcb.window,
            property: xcb.atoms.net_wm_allowed_actions,
            r#type: ATOM_ATOM,
            data: &[xcb.atoms.new_wm_action_close],
        })?;

        xcb.conn.send_and_check_request(&CreateGc {
            cid: xcb.gcontext,
            drawable: Drawable::Window(xcb.window),
            value_list: &[Gc::Background(xcb.screen.black_pixel()), Gc::GraphicsExposures(false)],
        })?;

        xcb.conn.send_and_check_request(&MapWindow { window: xcb.window })?;

        Ok(xcb)
    }

    fn set_window_shape_from_points(&self, from: DVec2, to: DVec2) -> VoidCookie {
        let rect_1 = Rectangle {
            x: (from.x - CONTROL_RADIUS) as i16,
            y: (from.y - CONTROL_RADIUS) as i16,
            width: (CONTROL_RADIUS * 2.0) as u16,
            height: (CONTROL_RADIUS * 2.0) as u16,
        };
        let rect_2 = Rectangle {
            x: (to.x - CONTROL_RADIUS) as i16,
            y: (to.y - CONTROL_RADIUS) as i16,
            ..rect_1
        };

        self.set_window_shape(shape::Sk::Input, &[rect_1, rect_2])
    }

    fn set_window_shape(&self, kind: shape::Sk, rectangles: &[Rectangle]) -> VoidCookie {
        self.conn.send_request(&shape::Rectangles {
            operation: shape::So::Set,
            destination_kind: kind,
            ordering: ClipOrdering::Unsorted,
            destination_window: self.window,
            x_offset: 0,
            y_offset: 0,
            rectangles,
        })
    }

    fn get_window_geometry(&self, window: Window) -> Result<WindowGeometry, Box<dyn Error>> {
        let cookie = self.conn.send_request(&GetGeometry {
            drawable: Drawable::Window(window),
        });
        let reply = self.conn.wait_for_reply(cookie)?;
        Ok(WindowGeometry { x: reply.x(), y: reply.y(), w: reply.width(), h: reply.height() })
    }
}

struct Render {
    surface: XCBSurface,
    ctx: Context,
}

impl Render {
    fn setup(xcb: &XCBObjects, height: u16, width: u16) -> Result<Render, Box<dyn Error>> {
        let surface = unsafe {
            let cairo_conn = XCBConnection::from_raw_none(xcb.conn.get_raw_conn() as *mut cairo::ffi::xcb_connection_t);
            let visual_type = XCBVisualType::from_raw_none(&xcb.visual_type as *const Visualtype as *mut cairo::ffi::xcb_visualtype_t);
            let drawable = XCBDrawable(xcb.window.resource_id());
            XCBSurface::create(&cairo_conn, &drawable, &visual_type, width as i32, height as i32)?
        };
        xcb.conn.flush()?;
        let cairo = Context::new(&surface)?;
        Ok(Render { ctx: cairo, surface })
    }

    fn resize(&self, width: i32, height: i32) -> Result<(), Box<dyn Error>> {
        self.surface.set_size(width, height)?;
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let xcb = Rc::new(XCBObjects::setup((INITIAL_LENGTH + RULER_HALF_WIDTH * 2.0) as u16, (RULER_HALF_WIDTH * 2.0) as u16)?);

    let root_geom = xcb.get_window_geometry(xcb.screen.root())?;

    let (mut from, mut to) = {
        let from_x = (root_geom.w as f64 - INITIAL_LENGTH) / 2.0 + RULER_HALF_WIDTH;
        let from_y = root_geom.h as f64 / 2.0 + RULER_HALF_WIDTH;

        (DVec2::new(from_x, from_y), DVec2::new(from_x + INITIAL_LENGTH, from_y))
    };

    let render = {
        let window_geom = compute_window_geometry(from, to);
        let render = Render::setup(&xcb, window_geom.w, window_geom.h)?;
        render
    };

    let mut dragging = Dragging::None;

    let mut last_update = Instant::now();

    let mut first = true;

    loop {
        let event = xcb.conn.wait_for_event()?;

        match event {
            xcb::Event::X(Event::Expose(_ev)) => {
                if first {
                    update(&xcb, &render, from, to, &mut last_update, true);
                    first = false;
                }
                redraw(&render, from, to)?;
                xcb.conn.flush()?;
            }
            xcb::Event::X(Event::ButtonPress(ev)) => {
                if ev.detail() == 1 {
                    let cursor = DVec2::new(ev.root_x() as f64, ev.root_y() as f64);
                    if cursor.distance_squared(from) < 6400.0 {
                        dragging = Dragging::From;
                    } else if cursor.distance_squared(to) < 6400.0 {
                        dragging = Dragging::To;
                    }
                }
            }
            xcb::Event::X(Event::MotionNotify(ev)) => {
                match dragging {
                    Dragging::From => {
                        let screen_size = DVec2::new(root_geom.w as f64, root_geom.h as f64);
                        let fix_distance = ev.state().intersects(KeyButMask::CONTROL);
                        let fix_angle = ev.state().intersects(KeyButMask::SHIFT);
                        handle_drag(&mut from, to, DVec2::new(ev.root_x() as f64, ev.root_y() as f64), screen_size, fix_distance, fix_angle);
                        if let Some(_) = update(&xcb, &render, from, to, &mut last_update, false) {
                            xcb.conn.flush()?;
                        }
                    }
                    Dragging::To => {
                        let screen_size = DVec2::new(root_geom.w as f64, root_geom.h as f64);
                        let fix_distance = ev.state().intersects(KeyButMask::CONTROL);
                        let fix_angle = ev.state().intersects(KeyButMask::SHIFT);
                        handle_drag(&mut to, from, DVec2::new(ev.root_x() as f64, ev.root_y() as f64), screen_size, fix_distance, fix_angle);
                        if let Some(_) = update(&xcb, &render, from, to, &mut last_update, false) {
                            xcb.conn.flush()?;
                        }
                    }
                    Dragging::None => {}
                }
            }
            xcb::Event::X(Event::ButtonRelease(ev)) => {
                if ev.detail() == 1 {
                    dragging = Dragging::None;
                    let pos = update(&xcb, &render, from, to, &mut last_update, true).unwrap().pos().as_dvec2();
                    xcb.set_window_shape_from_points(from - pos, to - pos);
                    xcb.conn.flush()?;
                }
            }
            xcb::Event::X(Event::KeyPress(ev)) => {
                if ev.detail() == 0x18 {
                    break Ok(());
                }
            }
            xcb::Event::X(Event::ClientMessage(ev)) => {
                if let ClientMessageData::Data32([atom, ..]) = ev.data() {
                    if atom == xcb.atoms.wm_del_window.resource_id() {
                        break Ok(());
                    }
                }
            }
            _ => {}
        }
    }
}

fn update(xcb: &XCBObjects, render: &Render, from: DVec2, to: DVec2, last_update: &mut Instant, force: bool) -> Option<(WindowGeometry)> {
    let now = Instant::now();
    if force || now - *last_update > Duration::from_millis(16) {
        let geometry = compute_window_geometry(from, to);
        render.resize(geometry.w as i32, geometry.h as i32);
        xcb.conn.send_request(&ConfigureWindow {
            window: xcb.window,
            value_list: &[
                ConfigWindow::X(geometry.x as i32),
                ConfigWindow::Y(geometry.y as i32),
                ConfigWindow::Width(geometry.w as u32),
                ConfigWindow::Height(geometry.h as u32)
            ],
        });
        *last_update = now;
        Some(geometry)
    } else {
        None
    }
}

fn redraw(render: &Render, from: DVec2, to: DVec2) -> Result<(), Box<dyn Error>> {
    let geometry = compute_window_geometry(from, to);
    let pos = geometry.pos().as_dvec2();
    draw(&render.ctx, from - pos, to - pos)?;
    Ok(())
}

fn compute_window_geometry(from: DVec2, to: DVec2) -> WindowGeometry {
    let min_x = from.x.min(to.x) - RULER_HALF_WIDTH;
    let max_x = from.x.max(to.x) + RULER_HALF_WIDTH;
    let min_y = from.y.min(to.y) - RULER_HALF_WIDTH;
    let max_y = from.y.max(to.y) + RULER_HALF_WIDTH;
    WindowGeometry {
        x: min_x as i16,
        y: min_y as i16,
        w: (max_x - min_x) as u16,
        h: (max_y - min_y) as u16,
    }
}

fn handle_drag(dragging: &mut DVec2, other: DVec2, cursor: DVec2, screen_size: DVec2, fix_distance: bool, fix_angle: bool) {
    let mut new_vec = cursor;

    if fix_distance {
        let new_diff_normalized = (new_vec - other).try_normalize().unwrap_or(DVec2::new(1.0, 0.0));
        let old_distance = dragging.distance(other);
        new_vec = other + new_diff_normalized * old_distance;

        new_vec = closest_point_below_line_on_circle(other, old_distance, DVec2::ZERO, DVec2::X, new_vec);
        new_vec = closest_point_below_line_on_circle(other, old_distance, screen_size, DVec2::X, new_vec);
        new_vec = closest_point_below_line_on_circle(other, old_distance, DVec2::ZERO, DVec2::Y, new_vec);
        new_vec = closest_point_below_line_on_circle(other, old_distance, screen_size, DVec2::Y, new_vec);
    }

    if fix_angle {
        let old_diff_normalized = (*dragging - other).try_normalize().unwrap_or(DVec2::X);
        new_vec = other + old_diff_normalized * new_vec.distance(other);
    }

    if other.distance_squared(new_vec) < MIN_LENGTH.powi(2) {
        let diff_normalized = (new_vec - other).try_normalize().unwrap_or(DVec2::X);
        new_vec = other + diff_normalized * MIN_LENGTH;
    }

    *dragging = new_vec.clamp(DVec2::ZERO, screen_size);
}

fn draw(ctx: &Context, from: DVec2, to: DVec2) -> Result<(), Box<dyn Error>> {
    let opacity = 0.6;
    let bg = 1.0;
    let accent = 0.7;

    ctx.set_operator(Operator::Source);
    ctx.set_source_rgba(0.0, 0.0, 0.0, 0.0);
    ctx.paint()?;

    ctx.save()?;

    ctx.set_line_width(2.0);

    ctx.translate(from.x, from.y);
    let angle = DVec2::X.angle_between(to - from);
    ctx.rotate(angle);

    let length = from.distance(to);
    let length_pixels = length as u32;

    ctx.rectangle(0.0, -RULER_HALF_WIDTH, length, RULER_HALF_WIDTH * 2.0);
    ctx.set_source_rgba(bg, bg, bg, opacity);
    ctx.fill()?;

    ctx.rectangle(0.0, -RULER_HALF_WIDTH, length, RULER_HALF_WIDTH * 2.0);
    ctx.set_source_rgba(accent, accent, accent, opacity);
    ctx.stroke()?;

    ctx.set_source_rgba(bg, bg, bg, opacity);

    ctx.arc(0.0, 0.0, CONTROL_RADIUS, 0.0, PI * 2.0);
    ctx.fill()?;

    ctx.arc(length, 0.0, CONTROL_RADIUS, 0.0, PI * 2.0);
    ctx.fill()?;

    ctx.set_source_rgba(accent, accent, accent, opacity);

    ctx.arc(0.0, 0.0, CONTROL_RADIUS, PI * 0.5, PI * 1.5);
    ctx.stroke()?;

    ctx.arc(length, 0.0, CONTROL_RADIUS, PI * 1.5, PI * 0.5);
    ctx.stroke()?;

    ctx.set_font_size(14.0);

    for i in (0..length_pixels).step_by(5) {
        let inner_width = RULER_HALF_WIDTH - match i % 50 {
            0 => 17.0,
            25 => 12.0,
            _ => 7.0
        };

        ctx.line_to(i as f64, -inner_width);
        ctx.line_to(i as f64, -RULER_HALF_WIDTH);
        ctx.stroke()?;
    }

    ctx.save()?;
    ctx.translate(30.0, RULER_HALF_WIDTH - 30.0);

    ctx.line_to(0.0, 0.0);
    ctx.line_to(30.0, 0.0);
    ctx.stroke()?;

    ctx.line_to(0.0, 0.0);
    let horizontal = DVec2::from_angle(angle) * 30.0;
    ctx.line_to(horizontal.x, -horizontal.y);
    ctx.stroke()?;

    ctx.arc(0.0, 0.0, 16.0, 0.0, -angle);
    ctx.stroke()?;

    let display_angle = if angle > 0.0 { PI * 2.0 - angle } else { angle.abs() } * 180.0 / PI;
    let angle_string = format!("{:.2}°", display_angle);
    let extents = ctx.text_extents(&angle_string)?;
    ctx.translate(35.0, extents.height());
    ctx.text_path(&angle_string);
    ctx.fill()?;
    ctx.restore()?;

    ctx.translate(50.0, -7.0);

    for i in (0..length_pixels).step_by(50).skip(1) {
        let str = i.to_string();
        let extents = ctx.text_extents(&str)?;
        ctx.translate(-extents.width() / 2.0, 0.0);
        ctx.text_path(&str);
        ctx.translate(50.0 + extents.width() / 2.0, 0.0);
        let visibility = ((length - i as f64) / 50.0).min(bg);
        let color = accent * visibility + bg * (1.0 - visibility);
        ctx.set_source_rgba(color, color, color, opacity);
        ctx.fill()?;
    }

    ctx.restore()?;

    Ok(())
}