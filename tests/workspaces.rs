//! The workspace selector and the offer to move the notes into diari/,
//! clicked with the pointer.

use diari::{AppWindow, WorkspaceItem};
use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

fn press(ui: &AppWindow, position: LogicalPosition) {
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
}

fn find(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label).next().unwrap_or_else(|| panic!("no {label:?}"))
}

fn click(ui: &AppWindow, label: &str) {
    let el = find(ui, label);
    let (p, s) = (el.absolute_position(), el.size());
    press(ui, LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.));
}

#[test]
fn switch_workspace_and_migrate() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(400., 800.));
    ui.show().unwrap();

    // Before workspaces: the old title and the offer to move the notes.
    ui.set_needs_migration(true);
    let migrated = Rc::new(RefCell::new(Vec::new()));
    let m = migrated.clone();
    ui.on_migrate(move || m.borrow_mut().push("now"));
    let m = migrated.clone();
    ui.on_migrate_later(move || m.borrow_mut().push("later"));
    click(&ui, "Later");
    click(&ui, "Move my notes");
    assert_eq!(*migrated.borrow(), vec!["later", "now"]);
    ui.set_needs_migration(false);

    // With workspaces: the title is a selector.
    let ws = vec![
        WorkspaceItem { folder: "diari".into(), name: "Diari".into(), color: slint::Color::from_rgb_u8(59, 130, 246) },
        WorkspaceItem { folder: "feina".into(), name: "Feina".into(), color: slint::Color::from_rgb_u8(34, 165, 91) },
    ];
    ui.set_workspaces(ModelRc::new(VecModel::from(ws)));
    ui.set_has_workspaces(true);
    ui.set_active_workspace("diari".into());
    ui.set_active_name("Diari".into());
    let picked = Rc::new(RefCell::new(Vec::<String>::new()));
    let p = picked.clone();
    ui.on_select_workspace(move |f| p.borrow_mut().push(f.to_string()));

    // The menu sits under the header; its rows report positions inside it.
    let button = find(&ui, "Workspace Diari");
    let (bp, bs) = (button.absolute_position(), button.size());
    click(&ui, "Workspace Diari");
    let origin = LogicalPosition::new(12., bp.y + bs.height + 4.);
    for (label, folder) in [("Feina", "feina"), ("All workspaces", "")] {
        if label != "Feina" {
            click(&ui, "Workspace Diari");
        }
        let row = ElementHandle::find_by_accessible_label(&ui, label).last().expect(label);
        let (rp, rs) = (row.absolute_position(), row.size());
        press(&ui, LogicalPosition::new(origin.x + rp.x + rs.width / 2., origin.y + rp.y + rs.height / 2.));
        assert_eq!(picked.borrow().last().map(String::as_str), Some(folder), "{label}");
    }
    let _ = bp.x;
}
