//! Clicks the note menu with the pointer, as a user would, and checks that
//! each entry does its job.

use diari::AppWindow;
use i_slint_backend_testing::ElementHandle;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::Cell;
use std::rc::Rc;

fn find(ui: &AppWindow, label: &str) -> ElementHandle {
    ElementHandle::find_by_accessible_label(ui, label)
        .next()
        .unwrap_or_else(|| panic!("no element labelled {label:?}"))
}

fn press_at(ui: &AppWindow, position: LogicalPosition) {
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
}

/// Click the middle of a header element.
fn click(ui: &AppWindow, label: &str) {
    let el = find(ui, label);
    let (p, s) = (el.absolute_position(), el.size());
    press_at(ui, LogicalPosition::new(p.x + s.width / 2., p.y + s.height / 2.));
}

/// Click a menu entry. Its position is reported relative to the menu,
/// which sits right-aligned under the header, 160px wide.
fn click_menu(ui: &AppWindow, label: &str) {
    let menu = find(ui, "Menu");
    let (m, ms) = (menu.absolute_position(), menu.size());
    let origin = LogicalPosition::new(m.x + ms.width - 160., m.y + ms.height + 4.);
    let el = find(ui, label);
    let (p, s) = (el.absolute_position(), el.size());
    press_at(ui, LogicalPosition::new(origin.x + p.x + s.width / 2., origin.y + p.y + s.height / 2.));
}

fn visible(ui: &AppWindow, label: &str) -> bool {
    ElementHandle::find_by_accessible_label(ui, label).next().is_some()
}

#[test]
fn menu_entries_work() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    ui.set_current_path("CONFIG.md".into());
    ui.set_current_is_config(true);
    ui.set_advanced(true);
    let checks = Rc::new(Cell::new(0));
    let deletes = Rc::new(Cell::new(0));
    let histories = Rc::new(Cell::new(0));
    let c = checks.clone();
    ui.on_check_config(move || c.set(c.get() + 1));
    let d = deletes.clone();
    ui.on_delete_note(move || d.set(d.get() + 1));
    let h = histories.clone();
    ui.on_open_history(move || h.set(h.get() + 1));

    // The header has only Preview and the menu (plus the note's name).
    assert!(visible(&ui, "Preview") && visible(&ui, "Menu"));
    assert!(!visible(&ui, "Rename") && !visible(&ui, "Delete") && !visible(&ui, "History"));

    // Check config
    click(&ui, "Menu");
    assert!(visible(&ui, "Check config"), "menu didn't open");
    click_menu(&ui, "Check config");
    assert_eq!(checks.get(), 1, "Check config did nothing");
    assert!(!visible(&ui, "Check config"), "menu stayed open");

    // History
    click(&ui, "Menu");
    click_menu(&ui, "History");
    assert_eq!(histories.get(), 1, "History did nothing");

    // Delete asks first, then deletes.
    click(&ui, "Menu");
    click_menu(&ui, "Delete");
    assert_eq!(deletes.get(), 0, "deleted without asking");
    assert!(visible(&ui, "Cancel"));
    click(&ui, "Cancel");
    assert_eq!(deletes.get(), 0);
    click(&ui, "Menu");
    click_menu(&ui, "Delete");
    click(&ui, "Yes, delete");
    assert_eq!(deletes.get(), 1, "Delete did nothing");

    // Without advanced mode there's no Delete; on other notes no Check config.
    ui.set_advanced(false);
    ui.set_current_is_config(false);
    click(&ui, "Menu");
    assert!(visible(&ui, "Rename"));
    assert!(!visible(&ui, "Delete") && !visible(&ui, "Check config"));
}
