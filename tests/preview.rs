//! Taps a task's checkbox in the preview, as a user would.

use diari::{AppWindow, MdBlock};
use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, VecModel};
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn task_checkbox_reports_its_position() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    ui.set_current_path("n.md".into());
    ui.set_preview(true);
    let task = MdBlock { kind: 13, text: "buy bread".into(), pos: 42, ..Default::default() };
    ui.set_blocks(ModelRc::new(VecModel::from(vec![task])));

    let got = Rc::new(Cell::new(-1));
    let g = got.clone();
    ui.on_toggle_task(move |p| g.set(p));

    let el = ElementHandle::find_by_accessible_label(&ui, "buy bread")
        .find(|e| e.accessible_checkable() == Some(true))
        .expect("no checkbox for the task");
    assert_eq!(el.accessible_checked(), Some(false));
    let (p, s) = (el.absolute_position(), el.size());
    let position = LogicalPosition::new(p.x + 10., p.y + s.height / 2.);
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    assert_eq!(got.get(), 42);
}
