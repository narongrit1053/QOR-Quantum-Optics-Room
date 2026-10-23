//! Stub until docs/M9-PLAN.md F lands (F's version replaces this file at merge).

use std::cell::RefCell;
use std::rc::Rc;

use lab_core::NodeId;
use lab_ui::AppWindow;

use crate::app_state::AppState;

pub fn open(_ui: &AppWindow, _app: &Rc<RefCell<AppState>>, _node: NodeId) {}
pub fn close(_ui: &AppWindow, _app: &Rc<RefCell<AppState>>) {}
