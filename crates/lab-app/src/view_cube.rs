//! M8 view cube: projects the six faces of a cube seen from the camera into 2D for Slint to draw,
//! and turns a click on a face into the camera direction that looks at it.
//!
//! Camera convention (as `ViewportEvent::CameraOrient` reports it, render space, Y up): the eye
//! sits at focus + d·(cos p sin y, sin p, cos p cos y). Render +X = room +X, render −Z = room +Y.
//! Front looks along room +Y, so it is the face with normal render +Z (yaw 0).

use std::cell::Cell;
use std::rc::Rc;

use lab_ui::{AppWindow, CubeFaceRow};
use lab_viewport::ViewportRequest;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use std::cell::RefCell;

use crate::app_state::AppState;

/// Edge length of the widget (logical px); matches `view_cube.slint`.
const SIZE: f32 = 96.0;
/// Pitch the Top/Bottom faces ease to (the camera's clamp).
const TOP_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
/// The default 3D view (camera.rs `spawn_camera`).
const HOME: (f32, f32) = (0.65, 0.55);

#[derive(Clone, Debug)]
pub struct CubeFace {
    pub label: &'static str,
    pub points: [(f32, f32); 4],
    pub front_facing: bool,
    /// How squarely the face looks at the camera (0..1); labels hide on steep faces.
    pub facing: f32,
    pub yaw: f32,
    pub pitch: f32,
}

type V = [f32; 3];

fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The six faces at camera orientation `yaw`/`pitch`, sorted back to front, in a `size`² box
/// (screen y down).
pub fn faces(yaw: f32, pitch: f32, size: f32) -> Vec<CubeFace> {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let back = [cp * sy, sp, cp * cy];
    let right = [cy, 0.0, -sy];
    // up = back × right
    let up = [back[1] * right[2] - back[2] * right[1], back[2] * right[0] - back[0] * right[2], back[0] * right[1] - back[1] * right[0]];
    let scale = size * 0.3;
    let project = |p: V| (size / 2.0 + dot(p, right) * scale, size / 2.0 - dot(p, up) * scale);
    // (label, normal, two in-plane axes, target direction)
    let defs: [(&str, V, V, V, (f32, f32)); 6] = [
        ("Front", [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], (0.0, 0.0)),
        ("Back", [0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], (std::f32::consts::PI, 0.0)),
        ("Right", [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], (std::f32::consts::FRAC_PI_2, 0.0)),
        ("Left", [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], (-std::f32::consts::FRAC_PI_2, 0.0)),
        // Top/Bottom keep the current heading.
        ("Top", [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], (yaw, TOP_PITCH)),
        ("Bottom", [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], (yaw, -TOP_PITCH)),
    ];
    let mut out: Vec<CubeFace> = defs
        .iter()
        .map(|&(label, n, a, b, (fy, fp))| {
            let corner = |s: f32, t: f32| project([n[0] + s * a[0] + t * b[0], n[1] + s * a[1] + t * b[1], n[2] + s * a[2] + t * b[2]]);
            let facing = dot(n, back);
            CubeFace {
                label,
                points: [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)],
                front_facing: facing > 1e-3,
                facing: facing.max(0.0),
                yaw: fy,
                pitch: fp,
            }
        })
        .collect();
    let depth = |f: &CubeFace| if f.front_facing { f.facing } else { -1.0 };
    out.sort_by(|a, b| depth(a).total_cmp(&depth(b)));
    out
}

/// The front-facing face under `(x, y)`: its index in `faces`.
fn hit_index(faces: &[CubeFace], x: f32, y: f32) -> Option<usize> {
    faces.iter().enumerate().rev().find(|(_, f)| f.front_facing && inside(&f.points, x, y)).map(|(i, _)| i)
}

/// The camera direction for a click at `(x, y)`, if it lands on a visible face.
pub fn hit(faces: &[CubeFace], x: f32, y: f32) -> Option<(f32, f32)> {
    hit_index(faces, x, y).map(|i| (faces[i].yaw, faces[i].pitch))
}

/// Point in a convex quad, either winding.
fn inside(q: &[(f32, f32); 4], x: f32, y: f32) -> bool {
    let side = |i: usize| {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)
    };
    let s: Vec<f32> = (0..4).map(side).collect();
    s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)
}

thread_local! {
    static ORIENT: Cell<(f32, f32)> = const { Cell::new(HOME) };
    static HOVER: Cell<Option<usize>> = const { Cell::new(None) };
}

fn refresh(ui: &AppWindow) {
    let (yaw, pitch) = ORIENT.get();
    let hover = HOVER.get();
    let rows: Vec<CubeFaceRow> = faces(yaw, pitch, SIZE)
        .into_iter()
        .enumerate()
        .filter(|(_, f)| f.front_facing)
        .map(|(i, f)| {
            let p = f.points;
            let commands = format!("M {} {} L {} {} L {} {} L {} {} Z", p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1, p[3].0, p[3].1);
            let cx = p.iter().map(|q| q.0).sum::<f32>() / 4.0;
            let cy = p.iter().map(|q| q.1).sum::<f32>() / 4.0;
            CubeFaceRow {
                label: SharedString::from(f.label),
                commands: commands.into(),
                cx,
                cy,
                show_label: f.facing > 0.35,
                hovered: hover == Some(i),
            }
        })
        .collect();
    ui.set_cube_faces(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// `ViewportEvent::CameraOrient`.
pub fn on_orient(ui: &AppWindow, yaw: f32, pitch: f32) {
    ORIENT.set((yaw, pitch));
    refresh(ui);
}

pub fn wire(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let ui_weak = ui.as_weak();
    ui.on_view_cube_hover(move |x, y| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let (yaw, pitch) = ORIENT.get();
        let h = (x >= 0.0).then(|| hit_index(&faces(yaw, pitch, SIZE), x, y)).flatten();
        if h != HOVER.get() {
            HOVER.set(h);
            refresh(&ui);
        }
    });
    let a = app.clone();
    ui.on_view_cube_clicked(move |x, y| {
        let (yaw, pitch) = ORIENT.get();
        if let Some((yaw, pitch)) = hit(&faces(yaw, pitch, SIZE), x, y) {
            a.borrow().send(ViewportRequest::ViewDirection { yaw, pitch });
        }
    });
    let a = app.clone();
    ui.on_view_cube_home(move || {
        a.borrow().send(ViewportRequest::ViewDirection { yaw: HOME.0, pitch: HOME.1 });
    });
    refresh(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_view_shows_front_and_hits_it() {
        let f = faces(0.0, 0.0, SIZE);
        let visible: Vec<_> = f.iter().filter(|f| f.front_facing).map(|f| f.label).collect();
        assert_eq!(visible, ["Front"]);
        assert_eq!(hit(&f, SIZE / 2.0, SIZE / 2.0), Some((0.0, 0.0)));
        assert_eq!(hit(&f, 1.0, 1.0), None);
        // From the home view, Front, Right and Top are visible; the frontmost is last.
        let f = faces(HOME.0, HOME.1, SIZE);
        let visible: Vec<_> = f.iter().filter(|f| f.front_facing).map(|f| f.label).collect();
        assert_eq!(visible.len(), 3);
        assert!(visible.contains(&"Top") && visible.contains(&"Front") && visible.contains(&"Right"));
    }
}
