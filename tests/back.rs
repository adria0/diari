//! Android's back gesture arrives as a Back key: it must reach the app
//! whatever was focused, including after the screen changed.

use diari::AppWindow;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::RefCell;
use std::rc::Rc;

fn press_back(ui: &AppWindow) {
    let k: slint::SharedString = Key::Back.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: k.clone() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: k });
}

#[test]
fn back_reaches_the_app() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(400., 800.)); // a phone: narrow
    ui.show().unwrap();

    let calls = Rc::new(RefCell::new(Vec::new()));
    let c = calls.clone();
    ui.on_back_pressed(move |wide| {
        c.borrow_mut().push(wide);
        true
    });

    // On the list.
    press_back(&ui);
    assert_eq!(*calls.borrow(), vec![false]);

    // In a note, after tapping into the editor (it takes the focus).
    ui.set_current_path("n.md".into());
    ui.set_editor_text("# n\n".into());
    let position = LogicalPosition::new(200., 400.);
    ui.window().dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    press_back(&ui);
    assert_eq!(calls.borrow().len(), 2);

    // The editor goes away (preview): the key must still arrive.
    ui.set_preview(true);
    press_back(&ui);
    assert_eq!(calls.borrow().len(), 3);
}
