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
    diari::richedit::install(&ui);
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

    // Typing a link reports the text and where the cursor is.
    let typed = Rc::new(RefCell::new(Vec::<(String, i32)>::new()));
    let t = typed.clone();
    ui.on_link_typed(move |text, cursor, _| t.borrow_mut().push((text.to_string(), cursor)));
    let picked = Rc::new(RefCell::new(Vec::<String>::new()));
    let p2 = picked.clone();
    ui.on_pick_link(move |n| p2.borrow_mut().push(n.to_string()));
    for c in " [[Mo".chars() {
        w.dispatch_event(WindowEvent::KeyPressed { text: c.to_string().into() });
        w.dispatch_event(WindowEvent::KeyReleased { text: c.to_string().into() });
    }
    let last = typed.borrow().last().cloned().unwrap();
    assert_eq!(last, ("# n\nhola [[Mo".to_string(), "# n\nhola [[Mo".len() as i32));

    // Tapping a suggestion picks it, and the editor keeps the focus.
    let names: Vec<slint::SharedString> = vec!["Montjoia".into()];
    ui.set_link_suggestions(slint::ModelRc::new(slint::VecModel::from(names)));
    let chip = i_slint_backend_testing::ElementHandle::find_by_accessible_label(&ui, "Montjoia")
        .next()
        .expect("no suggestion chip");
    let (cp, cs) = (chip.absolute_position(), chip.size());
    let at = LogicalPosition::new(cp.x + cs.width / 2., cp.y + cs.height / 2.);
    w.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
    assert_eq!(*picked.borrow(), vec!["Montjoia".to_string()]);
    w.dispatch_event(WindowEvent::KeyPressed { text: "x".into() });
    w.dispatch_event(WindowEvent::KeyReleased { text: "x".into() });
    assert!(ui.get_editor_text().ends_with('x'), "the editor lost the focus");
}
