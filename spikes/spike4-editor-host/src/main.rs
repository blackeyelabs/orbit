//! Spike 4's window: a Live Preview of a sample note (or the file given as
//! the first argument) rendered through Kit's `editor_host`. See
//! spikes/SPIKES.md for the checks.
//!
//! - `SPIKE4_TIMINGS=1` logs frame timings to stderr every two seconds.
//! - `SPIKE4_TYPE=1` types into the middle of the note, ten keys a second,
//!   as the 200 KB check does.

use std::{borrow::Cow, rc::Rc, time::Duration};

use gpui_kit::component::theme::{Theme, ThemeSet};
use gpui_kit::*;
use spike4_editor_host::host::{self, Editor};

/// The note shown when no file is given: every block the spike renders.
const SAMPLE: &str = "# Live Preview on editor_host

This paragraph has **bold**, *italic*, ~~struck~~, `code`, a [link](https://example.com) \
and a [[Wiki target|wikilink]]. It runs long enough to soft-wrap, so Up and Down have \
visual lines to move between. Move the caret into the syntax to reveal it.

## A list

- first item with **bold**
- second item
- [ ] a task
- [x] a done task

## Code

```rust
fn main() {
    println!(\"hello\");
}
```

---

## A table

| Name | Notes |
|---|---|
| alpha | has **bold** |
| beta |  |
| 日本語 | IME をここで試す |

日本語の **太字** と [[リンク]] で IME を試す。
";

/// Orbit's bundled fonts, which Orbit Paper names.
const FONTS: &[&[u8]] = &[
    include_bytes!("../../../crates/app/assets/fonts/PlusJakartaSans-Regular.ttf"),
    include_bytes!("../../../crates/app/assets/fonts/PlusJakartaSans-Italic.ttf"),
    include_bytes!("../../../crates/app/assets/fonts/PlusJakartaSans-Bold.ttf"),
    include_bytes!("../../../crates/app/assets/fonts/PlusJakartaSans-BoldItalic.ttf"),
    include_bytes!("../../../crates/app/assets/fonts/PlusJakartaSans-SemiBold.ttf"),
    include_bytes!("../../../crates/app/assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../../../crates/app/assets/fonts/JetBrainsMono-Bold.ttf"),
];

/// Orbit Paper and Orbit Paper Dark.
const THEME: &str = include_str!("../../../crates/app/assets/themes/orbit-paper.json");

actions!(spike4, [Quit]);

/// Opens the window over the sample note or the given file.
fn main() {
    let source = match std::env::args().nth(1) {
        Some(path) => std::fs::read_to_string(&path).expect("a readable note"),
        None => SAMPLE.to_string(),
    };
    application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        cx.text_system()
            .add_fonts(FONTS.iter().map(|f| Cow::Borrowed(*f)).collect())
            .expect("the bundled fonts load");
        let set: ThemeSet = serde_json::from_str(THEME).expect("Orbit Paper parses");
        Theme::update(cx, |theme| {
            for config in set.themes {
                theme.apply_config(&Rc::new(config));
            }
        });
        host::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("ctrl-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let bounds = Bounds::centered(None, size(px(760.), px(640.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    Theme::change(
                        gpui_kit::component::theme::ThemeMode::Light,
                        Some(window),
                        cx,
                    );
                    let editor = cx.new(|cx| Editor::new(&source, cx));
                    cx.new(|_| Shell { editor })
                },
            )
            .expect("open window");
        window
            .update(cx, |shell, window, cx| {
                let editor = shell.editor.clone();
                let focus = editor.read(cx).focus_handle().clone();
                window.focus(&focus, cx);
                cx.activate(true);
                if std::env::var_os("SPIKE4_TYPE").is_some() {
                    editor.update(cx, |editor, cx| {
                        let middle = editor.doc().leaves[editor.doc().leaves.len() / 2]
                            .source
                            .start;
                        editor.set_caret(middle, cx);
                    });
                    type_into(window, cx);
                }
                if std::env::var_os("SPIKE4_TIMINGS").is_some() {
                    log_timings(editor, window, cx);
                }
            })
            .ok();
    });
}

/// The window's root: the theme's background and font around the editor.
struct Shell {
    editor: Entity<Editor>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        div()
            .size_full()
            .bg(cx.theme().colors.background)
            .text_color(cx.theme().colors.foreground)
            .font_family("Plus Jakarta Sans")
            .text_size(px(16.))
            .child(self.editor.clone())
    }
}

/// Types a letter every 100 ms through the platform input path.
fn type_into(window: &mut Window, cx: &mut Context<Shell>) {
    cx.spawn_in(window, async move |_, cx| {
        for key in "the quick brown fox ".chars().cycle() {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            let key = if key == ' ' {
                "space".to_string()
            } else {
                key.to_string()
            };
            let typed = cx.update(|window, cx| {
                window.dispatch_keystroke(Keystroke::parse(&key).expect("a key"), cx);
            });
            if typed.is_err() {
                break;
            }
        }
    })
    .detach();
}

/// Logs, every two seconds: frames, the mean render, prepaint and whole
/// frame, the slowest frame, and the process's CPU time per second (user
/// plus system, as `scripts/measure.sh` counts it).
fn log_timings(editor: Entity<Editor>, window: &mut Window, cx: &mut Context<Shell>) {
    let timings = editor.read(cx).timings();
    cx.spawn_in(window, async move |_, cx| {
        let mut cpu = process_cpu();
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let t = timings.borrow_mut().take();
            let now = process_cpu();
            let cpu_ms_per_s = (now - cpu).as_secs_f64() * 1e3 / 2.;
            cpu = now;
            if t.frames > 0 {
                let ms = |d: Duration| d.as_secs_f64() * 1e3 / f64::from(t.frames);
                eprintln!(
                    "frames={} render_ms={:.2} prepaint_ms={:.2} frame_ms={:.2} \
                     slowest_frame_ms={:.2} cpu_ms_per_s={:.0}",
                    t.frames,
                    ms(t.render),
                    ms(t.prepaint),
                    ms(t.frame),
                    t.slowest_frame.as_secs_f64() * 1e3,
                    cpu_ms_per_s,
                );
            }
            if cx.update(|_, _| ()).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// The process's CPU time so far, user plus system, from /proc.
fn process_cpu() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Fields after the command name, which is in parentheses.
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .map_or(vec![], |(_, rest)| rest.split_whitespace().collect());
    // utime and stime are fields 14 and 15; the state, field 3, is index 0.
    let ticks: u64 = [11, 12]
        .iter()
        .filter_map(|&i| fields.get(i)?.parse::<u64>().ok())
        .sum();
    // Clock ticks are 100 a second on Linux.
    Duration::from_millis(ticks * 10)
}
