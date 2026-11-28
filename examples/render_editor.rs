//! Renders the app offscreen (software renderer, system fonts) to PNGs, to
//! check the styled editor without a screen or real notes:
//!   cargo run --example render_editor -- OUT_DIR [FONT] [SIZE] [NOTE.md]
//! (DARK=1 for the dark theme). Writes r1.png (the note), r2.png (after a
//! click and typing), r3.png (a selection across rows of different sizes),
//! r4.png and r5.png (a long note after Page Down and after scrolling).
use diari::{AppWindow, EditorFont};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel};
use slint::platform::{Key, Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, PlatformError};
use std::rc::Rc;

/// An RGB pixel for the software renderer.
#[derive(Clone, Copy, Default)]
struct Px([u8; 3]);
impl TargetPixel for Px {
    fn blend(&mut self, c: PremultipliedRgbaColor) {
        let a = 255 - c.alpha as u16;
        self.0[0] = (self.0[0] as u16 * a / 255) as u8 + c.red;
        self.0[1] = (self.0[1] as u16 * a / 255) as u8 + c.green;
        self.0[2] = (self.0[2] as u16 * a / 255) as u8 + c.blue;
    }
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Px([r, g, b])
    }
}
struct P(Rc<MinimalSoftwareWindow>);
impl Platform for P {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

const W: u32 = 1000;
const H: u32 = 700;
const SCALE: f32 = 2.0;

fn save(win: &MinimalSoftwareWindow, path: &str) {
    let (w, h) = ((W as f32 * SCALE) as u32, (H as f32 * SCALE) as u32);
    let mut buf = vec![Px::default(); (w * h) as usize];
    slint::platform::update_timers_and_animations();
    win.request_redraw();
    win.draw_if_needed(|r| {
        r.render(&mut buf, w as usize);
    });
    let raw: Vec<u8> = buf.iter().flat_map(|p| p.0).collect();
    image::RgbImage::from_raw(w, h, raw).unwrap().save(path).unwrap();
    println!("wrote {path}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or(".".into());
    let font = args.get(2).cloned().unwrap_or("Menlo".into());
    let size: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(14.);
    let win = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(P(win.clone()))).unwrap();
    let ui = AppWindow::new().unwrap();
    diari::richedit::install(&ui);
    ui.set_desktop(true);
    if std::env::var("DARK").is_ok() { ui.set_theme(2); }
    let f = ui.global::<EditorFont>();
    f.set_family(font.into());
    f.set_size(size);
    win.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: SCALE });
    win.set_size(PhysicalSize::new((W as f32 * SCALE) as u32, (H as f32 * SCALE) as u32));
    ui.show().unwrap();
    ui.set_current_path("diari/2026_11_28.md".into());
    let text = args.get(4).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| {
        "# 2026_11_28\n\n## Calendar\n- 09:30 Standup with **Marta**\n\n### To do\n- [x] Book *train* tickets\n- [ ] Review ***urgent*** stuff, `**not code**`\n\nSome **bold text**, some _italic text_ and a long line that should wrap nicely at the edge of the editor so we can see how wrapping behaves with styles and more words to make it wrap again.\nAccents: pa amb tomàquet, açò és fàcil 😀 done.\n".into()
    });
    ui.set_editor_text(text.into());
    save(&win, &format!("{out}/r1.png"));

    // Click at the end of the "Calendar" heading, type, select with shift+up.
    let click = |x: f32, y: f32| {
        let p = LogicalPosition::new(x, y);
        win.dispatch_event(WindowEvent::PointerPressed { position: p, button: PointerEventButton::Left });
        win.dispatch_event(WindowEvent::PointerReleased { position: p, button: PointerEventButton::Left });
    };
    let key = |t: slint::SharedString| {
        win.dispatch_event(WindowEvent::KeyPressed { text: t.clone() });
        win.dispatch_event(WindowEvent::KeyReleased { text: t });
    };
    click(900., 130.);
    for c in " **nou**".chars() {
        key(c.to_string().into());
    }
    save(&win, &format!("{out}/r2.png"));
    win.dispatch_event(WindowEvent::KeyPressed { text: Key::Shift.into() });
    key(Key::DownArrow.into());
    key(Key::DownArrow.into());
    key(Key::DownArrow.into());
    win.dispatch_event(WindowEvent::KeyReleased { text: Key::Shift.into() });
    save(&win, &format!("{out}/r3.png"));
    // A long note: page down three times, type, and scroll with the wheel.
    let long: String = (1..=300).map(|i| if i % 20 == 0 { format!("## Section {i}\n") } else { format!("line {i} with **bold** and *it*\n") }).collect();
    ui.set_editor_text(long.into());
    click(900., 130.);
    key(Key::PageDown.into());
    key(Key::PageDown.into());
    key(Key::PageDown.into());
    key("X".into());
    save(&win, &format!("{out}/r4.png"));
    win.dispatch_event(WindowEvent::PointerScrolled { position: LogicalPosition::new(900., 400.), delta_x: 0., delta_y: -3000. });
    for _ in 0..50 { slint::platform::update_timers_and_animations(); std::thread::sleep(std::time::Duration::from_millis(20)); }
    save(&win, &format!("{out}/r5.png"));
    let t = ui.get_editor_text();
    let line = t.lines().position(|l| l.contains('X'));
    println!("X typed on line {:?}", line.map(|l| l + 1));
}
