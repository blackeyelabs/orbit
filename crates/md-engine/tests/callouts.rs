//! Callouts (Obsidian) and alerts (GitHub): `> [!type] Title` blockquotes.

use md_engine::{Callout, CalloutKind, callout};

#[test]
fn a_callout_has_a_kind_a_title_and_a_markdown_body() {
    let quote = "> [!warning] Mind the **gap**\n> First line\n>\n> - a list\n";
    assert_eq!(
        callout(quote),
        Some(Callout {
            kind: CalloutKind::Warning,
            title: "Mind the **gap**".into(),
            body: "First line\n\n- a list\n".into(),
        })
    );
}

#[test]
fn github_alerts_and_obsidian_aliases_map_to_kinds_ignoring_case() {
    let kind = |q: &str| callout(q).map(|c| c.kind);
    assert_eq!(kind("> [!NOTE]\n> text"), Some(CalloutKind::Note));
    assert_eq!(kind("> [!TIP]\n> text"), Some(CalloutKind::Tip));
    assert_eq!(kind("> [!IMPORTANT]\n> text"), Some(CalloutKind::Tip));
    assert_eq!(kind("> [!CAUTION]\n> text"), Some(CalloutKind::Danger));
    assert_eq!(kind("> [!faq]\n> text"), Some(CalloutKind::Question));
    assert_eq!(kind("> [!done]\n> text"), Some(CalloutKind::Success));
    // Unknown types render like a note, as in Obsidian.
    assert_eq!(kind("> [!recipe]\n> text"), Some(CalloutKind::Note));
}

#[test]
fn without_a_title_the_type_names_the_callout() {
    assert_eq!(callout("> [!tldr]\n> short").unwrap().title, "Tldr");
    // A fold marker is accepted; folding itself is Phase 2.
    assert_eq!(
        callout("> [!info]- Details\n> hidden").unwrap().title,
        "Details"
    );
}

#[test]
fn an_ordinary_blockquote_is_not_a_callout() {
    assert_eq!(callout("> just a quote\n> [!note] later"), None);
    assert_eq!(callout("> [!] empty"), None);
}
