//! Fenced code blocks through the Document facade: their fences hidden
//! unless the caret is in the block, nothing inside them concealed, the
//! language read from the opening fence, and a new block made with Enter.
//! Expected values are worked out by hand.

use live_preview::Document;
use live_preview::blocks::{Code, Kind};

/// What each leaf shows.
fn shown(d: &Document) -> Vec<String> {
    d.views()
        .iter()
        .map(|v| v.display_text().to_string())
        .collect()
}

/// Each block's kind.
fn kinds(d: &Document) -> Vec<Kind> {
    d.blocks().blocks.iter().map(|b| b.kind.clone()).collect()
}

/// A code block in `lang`, closed or not.
fn code(lang: &str, closed: bool) -> Kind {
    code_closed_by(lang, closed.then_some("```"))
}

/// A code block in `lang`, closed by the fence `close` if any.
fn code_closed_by(lang: &str, close: Option<&str>) -> Kind {
    Kind::Code(Code {
        lang: (!lang.is_empty()).then(|| lang.to_string()),
        close: close.map(str::to_string),
    })
}

#[test]
fn fences_show_only_while_the_caret_is_in_the_block() {
    let src = "Intro\n\n```rust\nlet **x** = 1;\n```\n\nEnd";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    assert_eq!(kinds(&d)[1], code("rust", true));
    // Away from it: just the code, its markdown as typed.
    assert_eq!(shown(&d), ["Intro", "let **x** = 1;", "End"]);
    d.set_caret(src.find("x").unwrap());
    assert_eq!(shown(&d), ["Intro", "```rust\nlet **x** = 1;", "End"]);
}

#[test]
fn typing_in_a_code_block_never_conceals_markdown() {
    let src = "```\nfn main() {}\n```";
    let mut d = Document::new(src);
    d.set_caret(src.find("{").unwrap());
    d.type_text("**not bold** [[nor a link]]");
    d.move_doc_end(false);
    assert_eq!(shown(&d)[0], "fn main() **not bold** [[nor a link]]{}");
}

#[test]
fn backticks_then_enter_make_a_txt_block_with_the_caret_in_its_body() {
    let mut d = Document::new("Intro\n\n");
    d.move_doc_end(false);
    d.type_text("```");
    d.newline();
    assert_eq!(d.text(), "Intro\n\n```txt\n\n```");
    assert_eq!(d.caret(), "Intro\n\n```txt\n".len(), "in the body");
    assert_eq!(kinds(&d), [Kind::Paragraph, code("txt", true), Kind::Empty]);
    d.type_text("x");
    assert_eq!(d.text(), "Intro\n\n```txt\nx\n```");
    // A language typed already stays, and the text below stays out.
    let mut d = Document::new("\nbelow");
    d.type_text("```rust");
    d.newline();
    assert_eq!(d.text(), "```rust\n\n```\nbelow");
    assert_eq!(kinds(&d)[0], code("rust", true));
}

#[test]
fn enter_in_a_closed_block_s_fence_just_breaks_the_line() {
    let src = "```\nx\n```";
    let mut d = Document::new(src);
    d.set_caret(3);
    d.newline();
    assert_eq!(d.text(), "```\n\nx\n```");
}

#[test]
fn a_language_typed_after_the_fence_is_the_block_s() {
    let src = "```\nprint(1)\n```";
    let mut d = Document::new(src);
    d.set_caret(3);
    d.type_text("python");
    assert_eq!(kinds(&d)[0], code("python", true));
}

#[test]
fn a_chosen_language_is_written_to_the_fence_as_one_undo_step() {
    let src = "Intro\n\n```rust\nlet x = 1;\n```";
    let mut d = Document::new(src);
    d.move_doc_end(false);
    let caret = d.caret();
    assert!(d.set_code_language(1, "python"));
    assert_eq!(d.text(), "Intro\n\n```python\nlet x = 1;\n```");
    assert_eq!(d.caret(), caret + 2, "where it was in the text");
    assert!(d.undo());
    assert_eq!(d.text(), src);
    // A fence with no language gets one; a block that isn't code doesn't.
    let mut d = Document::new("```\nx\n```");
    assert!(d.set_code_language(0, "go"));
    assert_eq!(d.text(), "```go\nx\n```");
    assert!(!d.set_code_language(1, "go"));
}

#[test]
fn a_fence_closes_on_a_like_fence_at_least_as_long() {
    // Tildes fence too; a shorter run, another mark or an info string
    // after it doesn't close, and four spaces in isn't a fence.
    let src = "~~~~ python\n~~~\n```\n~~~~~\n\n    ```\nend";
    let d = Document::new(src);
    assert_eq!(kinds(&d)[0], code_closed_by("python", Some("~~~~~")));
    assert_eq!(kinds(&d)[1], Kind::Paragraph);
    let d = Document::new("```rust\n```js\n````");
    assert_eq!(kinds(&d)[0], code_closed_by("rust", Some("````")));
}

#[test]
fn backticks_typed_above_another_block_make_a_block_of_their_own() {
    // The new fence would pair with the closing fence below it.
    let mut d = Document::new("\n\n```js\nlet x;\n```");
    d.type_text("```");
    d.newline();
    assert_eq!(d.text(), "```txt\n\n```\n\n```js\nlet x;\n```");
    assert_eq!(kinds(&d)[0], code("txt", true));
    assert_eq!(kinds(&d)[1], code("js", true));
}
