//! Callouts (Obsidian) and alerts (GitHub): blockquotes whose first line is
//! `[!type]`, optionally followed by a fold marker and a title.

/// How a callout looks. Obsidian's aliases and GitHub's alert types map
/// onto these; unknown types are notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalloutKind {
    Note,
    Abstract,
    Info,
    Todo,
    Tip,
    Success,
    Question,
    Warning,
    Failure,
    Danger,
    Bug,
    Example,
    Quote,
}

impl CalloutKind {
    fn from_type(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "abstract" | "summary" | "tldr" => Self::Abstract,
            "info" => Self::Info,
            "todo" => Self::Todo,
            "tip" | "hint" | "important" => Self::Tip,
            "success" | "check" | "done" => Self::Success,
            "question" | "help" | "faq" => Self::Question,
            "warning" | "attention" => Self::Warning,
            "failure" | "fail" | "missing" => Self::Failure,
            "danger" | "error" | "caution" => Self::Danger,
            "bug" => Self::Bug,
            "example" => Self::Example,
            "quote" | "cite" => Self::Quote,
            _ => Self::Note,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Callout {
    pub kind: CalloutKind,
    /// Inline markdown; the type's name when the callout has no title.
    pub title: String,
    /// The callout's content as markdown, without the `>` markers.
    pub body: String,
}

/// Parses a blockquote's source as a callout, or `None` if it isn't one.
pub fn callout(blockquote: &str) -> Option<Callout> {
    let mut lines = blockquote.split_inclusive('\n').map(unquote);
    let first = lines.next()?.trim();
    let rest = first.strip_prefix("[!")?;
    let (name, after) = rest.split_once(']')?;
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let after = after.strip_prefix(['-', '+']).unwrap_or(after).trim();
    let title = if after.is_empty() {
        let mut chars = name.chars();
        let head = chars.next()?;
        head.to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    } else {
        after.to_string()
    };
    Some(Callout {
        kind: CalloutKind::from_type(name),
        title,
        body: lines.collect(),
    })
}

/// One line with its `>` marker and the single space after it removed.
fn unquote(line: &str) -> &str {
    let line = line.trim_start();
    let line = line.strip_prefix('>').unwrap_or(line);
    line.strip_prefix(' ').unwrap_or(line)
}
