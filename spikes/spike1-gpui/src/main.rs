//! Spike 1, GPUI half: a small editable, soft-wrapped note rendered through
//! the conceal model. Markdown syntax is hidden unless the caret or selection
//! touches it.
//!
//! Adapted from gpui-pre 0.3.9's `examples/input.rs`. The source text is the
//! model; every frame shapes `layout(source, selection).display_text()` and
//! maps caret, selection, mouse and IME positions through the layout's offset
//! map. Run with `cargo run -p spike1-gpui` and follow spikes/SPIKES.md.

use std::ops::Range;

use gpui_kit::*;
use spike1_conceal::{Layout, Selection, Style as RunStyle, layout};
use unicode_segmentation::UnicodeSegmentation as _;

actions!(
    spike1,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Copy,
        Quit
    ]
);

const SAMPLE: &str = "## Live Preview spike\n\
Here is **bold**, *italic*, ~~struck~~, `code`, [a link](https://example.com) and \
[[Wiki target|an alias]]. Keep typing past the end of this line so the paragraph soft-wraps, \
then move the caret with the arrow keys and the mouse.\n\
日本語の **太字** と [[リンク]] で IME を試す。";

struct ConcealInput {
    focus_handle: FocusHandle,
    source: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// What the last frame painted, for mapping mouse and IME positions.
    last_frame: Option<Frame>,
    is_selecting: bool,
}

/// One painted frame: the conceal layout and the shaped display text, one
/// shaped block per source line (hard line break).
struct Frame {
    conceal: Layout,
    blocks: Vec<ShapedBlock>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
}

struct ShapedBlock {
    /// Display offset of the block's first byte.
    start: usize,
    /// Offset of the block's top from the frame's top.
    top: Pixels,
    line: WrappedLine,
}

impl Frame {
    fn block_for_display(&self, display: usize) -> &ShapedBlock {
        self.blocks
            .iter()
            .rev()
            .find(|b| b.start <= display)
            .unwrap_or(&self.blocks[0])
    }

    fn source_index_for_point(&self, at: Point<Pixels>) -> usize {
        let local = at - self.bounds.origin;
        let block = self
            .blocks
            .iter()
            .rev()
            .find(|b| b.top <= local.y)
            .unwrap_or(&self.blocks[0]);
        let in_block = point(local.x, local.y - block.top);
        let ix = match block
            .line
            .closest_index_for_position(in_block, self.line_height)
        {
            Ok(ix) | Err(ix) => ix,
        };
        self.conceal.to_source(block.start + ix)
    }

    /// Top-left of a caret at `source`. Offsets beyond what this frame shaped
    /// (the source changed since it was painted) clamp to the end of the
    /// nearest block instead of jumping to the origin.
    fn point_for_source_index(&self, source: usize) -> Point<Pixels> {
        let display = self.conceal.to_display(source);
        let block = self.block_for_display(display);
        let ix = (display - block.start).min(block.line.len());
        let at = block
            .line
            .position_for_index(ix, self.line_height)
            .or_else(|| {
                block
                    .line
                    .position_for_index(block.line.len(), self.line_height)
            })
            .unwrap_or_default();
        self.bounds.origin + point(at.x, at.y + block.top)
    }
}

impl ConcealInput {
    fn selection(&self) -> Selection {
        let r = &self.selected_range;
        if self.selection_reversed {
            Selection {
                anchor: r.end,
                head: r.start,
            }
        } else {
            Selection {
                anchor: r.start,
                head: r.end,
            }
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    /// Up and Down move one visual (wrapped) line, keeping x.
    fn vertical(&mut self, lines: f32, cx: &mut Context<Self>) {
        let Some(frame) = &self.last_frame else {
            return;
        };
        let at = frame.point_for_source_index(self.cursor_offset());
        let target = point(
            at.x,
            at.y + frame.line_height * lines + frame.line_height / 2.,
        );
        let ix = frame.source_index_for_point(target);
        self.move_to(ix, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1., cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1., cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.source.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.source.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.source[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self
            .last_frame
            .as_ref()
            .map(|f| f.source_index_for_point(event.position))
        else {
            return;
        };
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(ix, cx)
        } else {
            self.move_to(ix, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting
            && let Some(ix) = self
                .last_frame
                .as_ref()
                .map(|f| f.source_index_for_point(event.position))
        {
            self.select_to(ix, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify()
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    /// The source range an IME or edit callback targets: the given UTF-16
    /// range, else the marked (preedit) text, else the selection.
    fn edit_range(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone())
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.source[..offset.min(self.source.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        utf16_to_utf8(&self.source, range.start)..utf16_to_utf8(&self.source, range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.source
            .grapheme_indices(true)
            .rev()
            .find_map(|(ix, _)| (ix < offset).then_some(ix))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.source
            .grapheme_indices(true)
            .find_map(|(ix, _)| (ix > offset).then_some(ix))
            .unwrap_or(self.source.len())
    }
}

impl EntityInputHandler for ConcealInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.source[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.edit_range(range_utf16.as_ref());
        self.source.replace_range(range.clone(), new_text);
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.edit_range(range_utf16.as_ref());
        self.source.replace_range(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        // The new selection is relative to the inserted text, in its UTF-16.
        self.selected_range = new_selected_range_utf16
            .map(|sel| {
                range.start + utf16_to_utf8(new_text, sel.start)
                    ..range.start + utf16_to_utf8(new_text, sel.end)
            })
            .unwrap_or_else(|| {
                let caret = range.start + new_text.len();
                caret..caret
            });
        self.selection_reversed = false;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let frame = self.last_frame.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = frame.point_for_source_index(range.start);
        let end = frame.point_for_source_index(range.end);
        Some(Bounds::from_corners(
            start,
            point(end.x.max(start.x + px(1.)), start.y + frame.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let ix = self.last_frame.as_ref()?.source_index_for_point(point);
        Some(self.offset_to_utf16(ix))
    }
}

/// The UTF-8 byte offset of UTF-16 offset `n` within `text`.
fn utf16_to_utf8(text: &str, n: usize) -> usize {
    let mut utf16 = 0;
    for (ix, ch) in text.char_indices() {
        if utf16 >= n {
            return ix;
        }
        utf16 += ch.len_utf16();
    }
    text.len()
}

/// Flattens possibly-overlapping conceal runs plus the IME marked range into
/// contiguous text runs covering the whole display text.
fn text_runs(conceal: &Layout, marked: Option<Range<usize>>, base: &TextStyle) -> Vec<TextRun> {
    let text_len = conceal.display_text().len();
    let runs = conceal.runs();
    let mut cuts: Vec<usize> = vec![0, text_len];
    for r in runs {
        cuts.extend([r.range.start, r.range.end]);
    }
    if let Some(m) = &marked {
        cuts.extend([m.start, m.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();

    let muted = hsla(0., 0., 0.5, 1.);
    let link = hsla(0.6, 0.6, 0.45, 1.);
    cuts.windows(2)
        .map(|w| {
            let seg = w[0]..w[1];
            let mut font = base.font();
            let mut color = base.color;
            let mut underline = None;
            let mut strikethrough = None;
            for r in runs
                .iter()
                .filter(|r| r.range.start <= seg.start && seg.end <= r.range.end)
            {
                match r.style {
                    RunStyle::Bold => font.weight = FontWeight::BOLD,
                    RunStyle::Italic => font.style = FontStyle::Italic,
                    RunStyle::Strike => {
                        strikethrough = Some(StrikethroughStyle {
                            thickness: px(1.),
                            color: None,
                        })
                    }
                    RunStyle::Code => font.family = "monospace".into(),
                    RunStyle::Link => {
                        color = link;
                        underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(link),
                            wavy: false,
                        });
                    }
                    // One shaped line has one font size, so headings are bold here.
                    RunStyle::Heading(_) => font.weight = FontWeight::BOLD,
                    RunStyle::Syntax => color = muted,
                }
            }
            if marked
                .as_ref()
                .is_some_and(|m| m.start <= seg.start && seg.end <= m.end)
            {
                underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                });
            }
            TextRun {
                len: seg.len(),
                font,
                color,
                background_color: None,
                underline,
                strikethrough,
            }
        })
        .filter(|r| r.len > 0)
        .collect()
}

struct ConcealElement {
    input: Entity<ConcealInput>,
}

struct Prepainted {
    frame: Frame,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for ConcealElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ConcealElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Prepainted>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui_kit::Style::default();
        style.size.width = relative(1.).into();
        // A fixed box is enough for a spike: room for several wrapped lines.
        style.size.height = (window.line_height() * 9.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Prepainted> {
        let input = self.input.read(cx);
        let conceal = layout(&input.source, input.selection());
        let marked = input
            .marked_range
            .as_ref()
            .map(|m| conceal.to_display(m.start)..conceal.to_display(m.end));
        let style = window.text_style();
        let runs = text_runs(&conceal, marked, &style);
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();
        let shaped = window
            .text_system()
            .shape_text(
                conceal.display_text().to_string().into(),
                font_size,
                &runs,
                Some(bounds.size.width),
                None,
            )
            .ok()?;
        // shape_text returns one wrapped line per hard line break.
        let mut blocks = Vec::new();
        let (mut start, mut top) = (0, px(0.));
        for line in shaped {
            let (len, height) = (line.len(), line.size(line_height).height);
            blocks.push(ShapedBlock { start, top, line });
            start += len + 1;
            top += height;
        }
        let frame = Frame {
            conceal,
            blocks,
            bounds,
            line_height,
        };

        let sel = input.selected_range.clone();
        let (cursor, selection) = if sel.is_empty() {
            let at = frame.point_for_source_index(sel.start);
            (
                Some(fill(Bounds::new(at, size(px(2.), line_height)), blue())),
                vec![],
            )
        } else {
            // One quad per visual line the selection crosses.
            let start = frame.point_for_source_index(sel.start);
            let end = frame.point_for_source_index(sel.end);
            let rows = ((end.y - start.y) / line_height).round() as usize;
            let quads = (0..=rows)
                .map(|row| {
                    let y = start.y + line_height * row as f32;
                    let x0 = if row == 0 { start.x } else { bounds.left() };
                    let x1 = if row == rows { end.x } else { bounds.right() };
                    fill(
                        Bounds::from_corners(point(x0, y), point(x1, y + line_height)),
                        rgba(0x3311ff30),
                    )
                })
                .collect();
            (None, quads)
        };
        Some(Prepainted {
            frame,
            cursor,
            selection,
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Option<Prepainted>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(mut pre) = prepaint.take() else {
            return;
        };
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for quad in pre.selection.drain(..) {
            window.paint_quad(quad);
        }
        let line_height = pre.frame.line_height;
        for block in &pre.frame.blocks {
            block
                .line
                .paint(
                    bounds.origin + point(px(0.), block.top),
                    line_height,
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                )
                .ok();
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = pre.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input
            .update(cx, |input, _| input.last_frame = Some(pre.frame));
    }
}

impl Render for ConcealInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let caret = self.cursor_offset();
        let display = self
            .last_frame
            .as_ref()
            .map(|f| f.conceal.to_display(caret));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .size_full()
            .bg(white())
            .text_color(black())
            .key_context("ConcealInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .text_size(px(20.))
            .line_height(px(30.))
            .child(ConcealElement { input: cx.entity() })
            // Debug readout for the checklist in SPIKES.md.
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_color(hsla(0., 0., 0.35, 1.))
                    .child(format!(
                        "caret source={caret} display={} selection={:?} marked={:?}",
                        display.map_or("-".into(), |d| d.to_string()),
                        self.selected_range,
                        self.marked_range
                    ))
                    .child(format!("source: {:?}", self.source)),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", Delete, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("up", Up, None),
            KeyBinding::new("down", Down, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new("ctrl-a", SelectAll, None),
            KeyBinding::new("ctrl-v", Paste, None),
            KeyBinding::new("ctrl-c", Copy, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let bounds = Bounds::centered(None, size(px(720.), px(460.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| ConcealInput {
                        focus_handle: cx.focus_handle(),
                        source: SAMPLE.to_string(),
                        selected_range: SAMPLE.len()..SAMPLE.len(),
                        selection_reversed: false,
                        marked_range: None,
                        last_frame: None,
                        is_selecting: false,
                    })
                },
            )
            .expect("open window");
        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle, cx);
                cx.activate(true);
            })
            .ok();
    });
}
