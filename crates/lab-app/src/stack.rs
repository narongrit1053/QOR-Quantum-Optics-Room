//! The beam-height stack builder (M2-PLAN POLA-46): pick an optic, a mount and a beam height;
//! the solver picks base, holder and post, and dragging the result places the whole stack.

use std::cell::RefCell;
use std::rc::Rc;

use lab_catalog::Catalog;
use lab_core::units::{format_length, format_length_with_unit, parse_length};
use lab_mates::solver::solve_stack;
use lab_ui::AppWindow;
use lab_viewport::{DragItem, StackSpec};
use slint::ComponentHandle;

use crate::app_state::AppState;

pub struct StackState {
    /// Catalog ids; `None` = mount only.
    pub optic: Option<String>,
    pub mount: Option<String>,
    pub beam_height: f64,
    /// The solved stack, or `None` when it does not fit.
    pub spec: Option<StackSpec>,
}

impl Default for StackState {
    fn default() -> Self {
        Self { optic: Some("generic:mirror-1in".into()), mount: Some("thorlabs:KM100".into()), beam_height: 75.0, spec: None }
    }
}

fn optics(catalog: &Catalog) -> Vec<Option<String>> {
    std::iter::once(None).chain(catalog.in_category("optics").into_iter().map(|d| Some(d.id.clone()))).collect()
}

fn mounts(catalog: &Catalog) -> Vec<Option<String>> {
    catalog.in_category("mounts").into_iter().map(|d| Some(d.id.clone())).collect()
}

/// `current` moved by `step` through `options`, wrapping; the first option when `current` is not one.
fn cycle(options: &[Option<String>], current: &Option<String>, step: i32) -> Option<String> {
    let n = options.len() as i32;
    if n == 0 {
        return None;
    }
    let at = options.iter().position(|o| o == current).map_or(0, |i| (i as i32 + step).rem_euclid(n));
    options[at as usize].clone()
}

fn name(catalog: &Catalog, id: &Option<String>, none: &str) -> String {
    id.as_ref().and_then(|id| catalog.get(id)).map_or(none.into(), |d| d.name.clone())
}

/// Solves the stack and shows the result.
pub fn fill(ui: &AppWindow, app: &mut AppState) {
    let catalog = app.catalog.clone();
    let units = app.units();
    let s = &mut app.stack;
    if s.mount.as_ref().and_then(|m| catalog.get(m)).is_none() {
        s.mount = mounts(&catalog).into_iter().next().flatten();
    }
    let candidates = catalog.in_category("posts");
    let solved = s.mount.as_ref().and_then(|m| catalog.get(m)).map(|m| solve_stack(m, s.beam_height, &candidates));
    let (spec, text) = match solved {
        None => (None, "The library has no mounts.".to_string()),
        Some(Err(no_fit)) => (None, no_fit.to_string()),
        Some(Ok(sol)) => {
            let label = |id: &str| catalog.get(id).map_or(id.to_string(), |d| d.name.clone());
            let base = sol.base.as_deref().map(|b| format!(" on {}", label(b))).unwrap_or_default();
            let text = format!(
                "{} in {}{} · {} margin",
                label(&sol.post),
                label(&sol.holder),
                base,
                format_length_with_unit(sol.margin, units)
            );
            let spec = StackSpec {
                optic: s.optic.clone(),
                mount: s.mount.clone().unwrap_or_default(),
                base: sol.base,
                holder: sol.holder,
                post: sol.post,
                slide: sol.slide,
            };
            (Some(spec), text)
        }
    };
    s.spec = spec;
    ui.set_stack_optic(name(&catalog, &s.optic, "No optic").into());
    ui.set_stack_mount(name(&catalog, &s.mount, "No mount").into());
    ui.set_stack_height(format_length(s.beam_height, units).into());
    ui.set_stack_unit(units.suffix().into());
    ui.set_stack_result(text.into());
    ui.set_stack_fits(s.spec.is_some());
}

pub fn connect(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    let (ui_weak, a) = (ui.as_weak(), app.clone());
    ui.on_stack_pick(move |which, step| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let mut app = a.borrow_mut();
        let catalog = app.catalog.clone();
        if which == "optic" {
            app.stack.optic = cycle(&optics(&catalog), &app.stack.optic, step);
        } else {
            app.stack.mount = cycle(&mounts(&catalog), &app.stack.mount, step);
        }
        fill(&ui, &mut app);
    });

    let (ui_weak, a) = (ui.as_weak(), app.clone());
    ui.on_stack_height_committed(move |text| {
        let Some(ui) = ui_weak.upgrade() else { return false };
        let mut app = a.borrow_mut();
        let units = app.units();
        match parse_length(&text, units) {
            Ok(h) if h > 0.0 => {
                app.stack.beam_height = h;
                fill(&ui, &mut app);
                true
            }
            _ => false,
        }
    });

    let (ui_weak, a) = (ui.as_weak(), app.clone());
    ui.on_stack_drag(move |kind, x, y| {
        let Some(ui) = ui_weak.upgrade() else { return };
        let Some(spec) = a.borrow().stack.spec.clone() else { return };
        crate::library::drag(&ui, &a, DragItem::Stack(spec), kind, x, y);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KM100 + 1" mirror at 75 mm: five parts, mirror centre 75 ± 0.1 mm above the table; 500 mm does not fit.
    #[test]
    fn km100_stack_reaches_75mm() {
        let (catalog, _) = Catalog::load_default();
        let candidates = catalog.in_category("posts");
        let mount = catalog.get("thorlabs:KM100").unwrap();
        assert!(solve_stack(mount, 500.0, &candidates).is_err());
        let sol = solve_stack(mount, 75.0, &candidates).unwrap();
        let spec = StackSpec {
            optic: Some("generic:mirror-1in".into()),
            mount: mount.id.clone(),
            base: sol.base.clone(),
            holder: sol.holder,
            post: sol.post,
            slide: sol.slide,
        };
        let root = catalog.get(DragItem::Stack(spec.clone()).root()).unwrap();
        let (mut p, mut r) = lab_mates::seat_pose(root, Default::default(), 0.0);
        let chain = lab_viewport::stack_chain(&catalog, &spec).unwrap();
        assert_eq!(chain.len() + 1, if sol.base.is_some() { 5 } else { 4 });
        for (_, _, position, rotation) in chain {
            p += r * position;
            r *= rotation;
        }
        assert!((p.z - 75.0).abs() < 0.1, "mirror centre at {}", p.z);
    }

    #[test]
    fn cycle_wraps() {
        let options = vec![None, Some("a".to_string()), Some("b".to_string())];
        assert_eq!(cycle(&options, &None, -1), Some("b".into()));
        assert_eq!(cycle(&options, &Some("b".into()), 1), None);
        assert_eq!(cycle(&options, &Some("x".into()), 1), None);
    }
}
