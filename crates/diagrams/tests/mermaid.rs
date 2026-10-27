//! Mermaid through the DiagramRenderer trait, with Spike 2's corpus
//! (spikes/spike2-mermaid/corpus, the user's own diagrams in `user/`) as
//! golden inputs: every guaranteed type draws in both themes, broken input
//! errors with its line instead of panicking, and the cache renders each
//! diagram once, off the caller's thread.

use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use diagrams::{Diagram, DiagramCache, DiagramError, DiagramRenderer, DiagramTheme, Mermaid};

const THEMES: [DiagramTheme; 2] = [DiagramTheme::Light, DiagramTheme::Dark];

fn corpus() -> Vec<(String, String)> {
    let dir = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../spikes/spike2-mermaid/corpus"
    ));
    let mut found = Vec::new();
    for dir in [dir.to_path_buf(), dir.join("user")] {
        let mut paths: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "mmd"))
            .collect();
        paths.sort();
        for path in paths {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            found.push((name, fs::read_to_string(&path).unwrap()));
        }
    }
    found
}

#[test]
fn every_corpus_diagram_draws_in_both_themes() {
    let mermaid = Mermaid::new();
    let mut failures = Vec::new();
    for (name, source) in corpus()
        .into_iter()
        .filter(|(name, _)| name != "15-invalid.mmd")
    {
        for theme in THEMES {
            match mermaid.render(&source, theme) {
                Ok(Diagram { svg, width, height }) => {
                    assert!(svg.starts_with("<svg"), "{name}");
                    assert!(width > 0. && height > 0., "{name}: {width}×{height}");
                    // Sized for drawing, not for a web page.
                    assert!(!svg[..svg.find('>').unwrap()].contains("100%"), "{name}");
                }
                Err(e) => failures.push(format!("{name} ({theme:?}): {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_guaranteed_types_are_among_those_supported() {
    let supported = Mermaid::new().supported();
    for kind in [
        "flowchart",
        "sequence",
        "class",
        "er",
        "state",
        "gantt",
        "mindmap",
        "pie",
        // Widened in Phase 0 to what the user's diagrams use.
        "timeline",
        "quadrantchart",
        "swimlane",
        "requirement",
        "xychart",
        "block",
        "journey",
        "gitgraph",
    ] {
        assert!(
            supported.iter().any(|s| s == kind),
            "{kind} in {supported:?}"
        );
    }
}

#[test]
fn labels_in_any_script_are_kept() {
    let source = "flowchart LR\n  A[日本語のラベル] --> B[Ünïcödé]\n";
    let diagram = Mermaid::new().render(source, DiagramTheme::Dark).unwrap();
    assert!(diagram.svg.contains("日本語のラベル"));
    assert!(diagram.svg.contains("Ünïcödé"));
}

#[test]
fn a_syntax_error_says_which_line() {
    let invalid = corpus()
        .into_iter()
        .find(|(name, _)| name == "15-invalid.mmd")
        .unwrap()
        .1;
    match Mermaid::new().render(&invalid, DiagramTheme::Light) {
        Err(DiagramError::Syntax { line, message }) => {
            assert_eq!(line, Some(2));
            assert!(message.contains("Unterminated"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    match Mermaid::new().render(
        "sequenceDiagram\n  Alice->>Bob: hi\n  Bob-->>\n",
        DiagramTheme::Light,
    ) {
        Err(DiagramError::Syntax { line, .. }) => assert_eq!(line, Some(3)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_unknown_type_is_unsupported() {
    for source in ["notadiagram foo\n", "", "   \n"] {
        assert!(
            matches!(
                Mermaid::new().render(source, DiagramTheme::Light),
                Err(DiagramError::Unsupported)
            ),
            "{source:?}"
        );
    }
}

#[test]
fn broken_input_errors_and_never_panics() {
    for source in [
        "flowchart LR\n  A -->",
        "graph TD\n  A[[[[[[",
        "pie\n  \"a\" : -5\n  \"b\" : abc",
        "gantt\n  dateFormat YYYY\n  section\n  task :a, 9999-99-99, 1d",
        "classDiagram\n  class A {\n",
        "%%{init: {\"theme\": ",
        "erDiagram\n  A ||--o{ B :",
        "stateDiagram-v2\n  [*] --> [*] --> \n",
        "\u{0}\u{1}flowchart",
    ] {
        let _ = Mermaid::new().render(source, DiagramTheme::Light);
    }
}

#[test]
fn a_render_that_takes_too_long_gives_up() {
    let large = corpus()
        .into_iter()
        .find(|(name, _)| name == "16-flowchart-large.mmd")
        .unwrap()
        .1;
    let hurried = Mermaid::new().time_limit(Duration::ZERO);
    assert!(matches!(
        hurried.render(&large, DiagramTheme::Light),
        Err(DiagramError::TimedOut)
    ));
}

/// Mermaid, counting how often it's asked.
struct Counting(Mermaid, AtomicUsize);

impl DiagramRenderer for Counting {
    fn supported(&self) -> Vec<String> {
        self.0.supported()
    }

    fn render(&self, source: &str, theme: DiagramTheme) -> Result<Diagram, DiagramError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.render(source, theme)
    }
}

#[test]
fn the_cache_renders_off_the_callers_thread_once_per_source_and_theme() {
    let renderer = Arc::new(Counting(Mermaid::new(), AtomicUsize::new(0)));
    let (tx, rx) = mpsc::channel();
    let cache = DiagramCache::new(renderer.clone(), move || {
        tx.send(()).ok();
    });
    let source = "flowchart LR\n  A --> B\n";
    // Not ready yet: it's being drawn in the background.
    assert!(cache.get(source, DiagramTheme::Light).is_none());
    rx.recv_timeout(Duration::from_secs(30))
        .expect("told when it's ready");
    let drawn = cache.get(source, DiagramTheme::Light).expect("ready now");
    assert!(drawn.is_ok());
    assert!(cache.get(source, DiagramTheme::Light).is_some());
    assert_eq!(renderer.1.load(Ordering::SeqCst), 1);

    // Another theme is another drawing; errors are kept too.
    assert!(cache.get(source, DiagramTheme::Dark).is_none());
    assert!(
        cache
            .get("flowchart LR\n  A -->", DiagramTheme::Dark)
            .is_none()
    );
    rx.recv_timeout(Duration::from_secs(30)).unwrap();
    rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        cache
            .get("flowchart LR\n  A -->", DiagramTheme::Dark)
            .unwrap()
            .is_err()
    );
    assert_eq!(renderer.1.load(Ordering::SeqCst), 3);
}

#[test]
fn a_diagram_may_start_with_its_own_front_matter() {
    let source = "---\ntitle: Launch\n---\nflowchart LR\n  A --> B\n";
    let diagram = Mermaid::new().render(source, DiagramTheme::Dark).unwrap();
    assert!(diagram.svg.contains("Launch"));
    // Lines still count from the top of what was written.
    let broken = "---\ntitle: Launch\n---\nflowchart LR\n  A[open\n";
    match Mermaid::new().render(broken, DiagramTheme::Light) {
        Err(DiagramError::Syntax { line, .. }) => assert_eq!(line, Some(5)),
        other => panic!("{other:?}"),
    }
}
