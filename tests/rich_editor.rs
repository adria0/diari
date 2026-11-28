//! The styled editor: markdown shown bold, italic and with bigger headings,
//! and the cursor following that layout. Uses the testing backend's fixed
//! font (every byte one font-size wide), so positions are exact.

use diari::{AppWindow, EditorFont};
use i_slint_backend_testing::ElementQuery;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, SharedString};

/// A drawn run: position relative to the first, width (its columns) and
/// height (its row; the fixed font's line height is its size).
#[derive(Debug, PartialEq)]
struct Run {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn setup(text: &str) -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    diari::richedit::install(&ui);
    let font = ui.global::<EditorFont>();
    font.set_family("FixedTestFont".into());
    font.set_size(10.);
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    ui.set_current_path("n.md".into());
    ui.set_editor_text(text.into());
    ui
}

/// The drawn runs, relative to the first one's row.
fn runs(ui: &AppWindow) -> Vec<Run> {
    let mut v: Vec<Run> = ElementQuery::from_root(ui)
        .match_type_name("RunText")
        .find_all()
        .into_iter()
        .map(|e| {
            let (p, s) = (e.absolute_position(), e.size());
            // The run's width has one font size of slack.
            Run { x: p.x, y: p.y, w: s.width - s.height, h: s.height }
        })
        .collect();
    v.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
    if let Some((x0, y0)) = v.first().map(|r| (r.x, r.y)) {
        for r in &mut v {
            r.x -= x0;
            r.y -= y0;
        }
    }
    v
}

fn key(ui: &AppWindow, text: impl Into<SharedString>) {
    let t = text.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: t });
}

fn type_str(ui: &AppWindow, s: &str) {
    for c in s.chars() {
        key(ui, c.to_string());
    }
}

fn click(ui: &AppWindow, at: LogicalPosition) {
    let w = ui.window();
    w.dispatch_event(WindowEvent::PointerPressed { position: at, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerReleased { position: at, button: PointerEventButton::Left });
}

fn run_origin(ui: &AppWindow) -> LogicalPosition {
    ElementQuery::from_root(ui)
        .match_type_name("RunText")
        .find_all()
        .into_iter()
        .map(|e| e.absolute_position())
        .min_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap())
        .unwrap()
}

#[test]
fn shows_markdown_styled() {
    let ui = setup("# Title\nsome **bold** and *it*\n## Sub");
    let r = runs(&ui);
    let run = |x, y, w, h| Run { x, y, w, h };
    assert_eq!(
        r,
        vec![
            // Heading 1: 1.5 times the size, so 15px columns and rows.
            run(0., 0., 15., 15.),  // #
            run(15., 0., 90., 15.), // " Title"
            // Body: 10px. Markers and styled text are separate runs.
            run(0., 15., 50., 10.),   // "some "
            run(50., 15., 20., 10.),  // **
            run(70., 15., 40., 10.),  // bold
            run(110., 15., 20., 10.), // **
            run(130., 15., 50., 10.), // " and "
            run(180., 15., 10., 10.), // *
            run(190., 15., 20., 10.), // it
            run(210., 15., 10., 10.), // *
            // Heading 2: 1.3 times.
            run(0., 25., 26., 13.), // ##
            run(26., 25., 52., 13.), // " Sub"
        ]
    );
}

#[test]
fn typing_clicking_and_moving() {
    let ui = setup("# Title\nab");
    let o = run_origin(&ui);
    // Click right of "ab", on the body row under the 15px heading row.
    click(&ui, LogicalPosition::new(o.x + 100., o.y + 18.));
    type_str(&ui, "**x**");
    assert_eq!(ui.get_editor_text(), "# Title\nab**x**");
    // "x" is drawn on its own (bold), after "ab" and "**".
    assert_eq!(runs(&ui).iter().filter(|r| r.y == 15.).map(|r| r.x).collect::<Vec<_>>(), vec![0., 20., 40., 50.]);

    // Up from column 7 (x=70): the heading's columns are 15px, 70/15 = 4.67,
    // so before its 5th character.
    key(&ui, Key::UpArrow);
    type_str(&ui, "|");
    assert_eq!(ui.get_editor_text(), "# Tit|le\nab**x**");
    // Down: back to x=70 on the body row: column 7, its end.
    key(&ui, Key::DownArrow);
    type_str(&ui, "!");
    assert_eq!(ui.get_editor_text(), "# Tit|le\nab**x**!");
    // Home / End move within the row.
    key(&ui, Key::Home);
    type_str(&ui, "<");
    key(&ui, Key::End);
    type_str(&ui, ">");
    assert_eq!(ui.get_editor_text(), "# Tit|le\n<ab**x**!>");
    // Shift+Up selects up to the same x in the heading; typing replaces it.
    let w = ui.window();
    w.dispatch_event(WindowEvent::KeyPressed { text: Key::Shift.into() });
    key(&ui, Key::UpArrow);
    w.dispatch_event(WindowEvent::KeyReleased { text: Key::Shift.into() });
    type_str(&ui, "Z");
    assert_eq!(ui.get_editor_text(), "# Tit|lZ");

    // Click in the heading row at 2.4 columns of 15px: before the 3rd char.
    click(&ui, LogicalPosition::new(o.x + 2.4 * 15., o.y + 5.));
    type_str(&ui, "^");
    assert_eq!(ui.get_editor_text(), "# ^Tit|lZ");

    // Up/down keep the column across rows of other sizes.
    ui.set_editor_text("abcdefgh\n# H\nabcdefgh".into());
    click(&ui, LogicalPosition::new(run_origin(&ui).x + 60., run_origin(&ui).y + 3.));
    key(&ui, Key::DownArrow);
    key(&ui, Key::DownArrow);
    type_str(&ui, "_");
    assert_eq!(ui.get_editor_text(), "abcdefgh\n# H\nabcdef_gh");
}

#[test]
fn proportional_font_falls_back_to_plain() {
    i_slint_backend_testing::init_no_event_loop();
    let ui = AppWindow::new().unwrap();
    diari::richedit::install(&ui);
    // Not the fixed test font: measured for real; a proportional face.
    ui.global::<EditorFont>().set_family("Helvetica".into());
    ui.window().set_size(slint::LogicalSize::new(1200., 800.));
    ui.show().unwrap();
    ui.set_current_path("n.md".into());
    ui.set_editor_text("# **x**".into());
    assert!(runs(&ui).is_empty());
}

#[test]
fn double_click_selects_a_word() {
    let ui = setup("hola **món** adéu");
    let o = run_origin(&ui);
    // On "món" (columns 7-9 of 10px; "ó" is 2 bytes but one column).
    let at = LogicalPosition::new(o.x + 81., o.y + 5.);
    click(&ui, at);
    click(&ui, at);
    type_str(&ui, "X");
    assert_eq!(ui.get_editor_text(), "hola **X** adéu");
}

#[test]
fn dragging_selects() {
    let ui = setup("# Big\nsmall text");
    let o = run_origin(&ui);
    let w = ui.window();
    // From column 1 of the heading (15px columns) to column 5 of the body row.
    let from = LogicalPosition::new(o.x + 15., o.y + 5.);
    let to = LogicalPosition::new(o.x + 50., o.y + 18.);
    w.dispatch_event(WindowEvent::PointerPressed { position: from, button: PointerEventButton::Left });
    w.dispatch_event(WindowEvent::PointerMoved { position: to });
    w.dispatch_event(WindowEvent::PointerReleased { position: to, button: PointerEventButton::Left });
    type_str(&ui, "-");
    assert_eq!(ui.get_editor_text(), "#- text");
}
