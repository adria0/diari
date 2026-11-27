//! Types into the note editor, as a user would, and checks the text arrives.

use diari::AppWindow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn typing_reaches_the_note() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    ui.set_current_path("n.md".into());
    ui.set_editor_text("# n\n".into());

    let edits = Rc::new(RefCell::new(Vec::<String>::new()));
    let e = edits.clone();
    ui.on_text_edited(move |t| e.borrow_mut().push(t.to_string()));

    let editor = i_slint_backend_testing::ElementQuery::from_root(&ui)
        .match_accessible_role(i_slint_backend_testing::AccessibleRole::TextInput)
        .find_all()
        .into_iter()
        .find(|e| e.accessible_value().as_deref() == Some("# n\n"))
        .expect("no note editor");
    let (p, s) = (editor.absolute_position(), editor.size());
    let position = LogicalPosition::new(p.x + s.width / 2., p.y + s.height - 20.);
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerPressed { position, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position, button: PointerEventButton::Left });
    for c in "hola".chars() {
        w.dispatch_event(WindowEvent::KeyPressed { text: c.to_string().into() });
        w.dispatch_event(WindowEvent::KeyReleased { text: c.to_string().into() });
    }
    assert_eq!(ui.get_editor_text(), "# n\nhola");
    assert_eq!(edits.borrow().last().map(String::as_str), Some("# n\nhola"));
}
