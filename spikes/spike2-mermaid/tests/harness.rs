use std::{sync::Arc, time::Duration};

use spike2_mermaid::{
    Diagram, DiagramRenderer, Outcome, Status, Theme, gallery_html, png_path, results_csv, run,
    svg_path,
};

struct Echo;
impl DiagramRenderer for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn render(&self, source: &str, _theme: Theme) -> Result<String, String> {
        Ok(format!("<svg>{source}</svg>"))
    }
}

fn corpus() -> Vec<Diagram> {
    vec![Diagram {
        name: "flow".into(),
        source: "flowchart TD\nA-->B".into(),
    }]
}

const LIMIT: Duration = Duration::from_secs(2);

#[test]
fn a_successful_render_is_ok_and_keeps_its_svg() {
    let out = run(&[Arc::new(Echo)], &corpus(), &[Theme::Light], LIMIT);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].renderer, "echo");
    assert_eq!(out[0].diagram, "flow");
    assert_eq!(out[0].status, Status::Ok);
    assert_eq!(
        out[0].svg.as_deref(),
        Some("<svg>flowchart TD\nA-->B</svg>")
    );
}

struct Refuses;
impl DiagramRenderer for Refuses {
    fn name(&self) -> &str {
        "refuses"
    }
    fn render(&self, _: &str, _: Theme) -> Result<String, String> {
        Err("unsupported diagram type".into())
    }
}

#[test]
fn a_render_error_is_recorded_with_its_message() {
    let out = run(&[Arc::new(Refuses)], &corpus(), &[Theme::Light], LIMIT);
    assert_eq!(
        out[0].status,
        Status::Error("unsupported diagram type".into())
    );
    assert_eq!(out[0].svg, None);
}

struct Panics;
impl DiagramRenderer for Panics {
    fn name(&self) -> &str {
        "panics"
    }
    fn render(&self, _: &str, _: Theme) -> Result<String, String> {
        panic!("layout exploded")
    }
}

#[test]
fn a_panic_is_caught_and_the_run_continues() {
    let out = run(
        &[Arc::new(Panics), Arc::new(Echo)],
        &corpus(),
        &[Theme::Light],
        LIMIT,
    );
    assert_eq!(out[0].status, Status::Panic("layout exploded".into()));
    assert_eq!(out[1].status, Status::Ok);
}

struct Hangs;
impl DiagramRenderer for Hangs {
    fn name(&self) -> &str {
        "hangs"
    }
    fn render(&self, _: &str, _: Theme) -> Result<String, String> {
        std::thread::sleep(Duration::from_secs(60));
        Ok(String::new())
    }
}

#[test]
fn a_render_past_the_limit_times_out_and_the_run_continues() {
    let started = std::time::Instant::now();
    let out = run(
        &[Arc::new(Hangs), Arc::new(Echo)],
        &corpus(),
        &[Theme::Light],
        Duration::from_millis(200),
    );
    assert_eq!(out[0].status, Status::Timeout);
    assert_eq!(out[1].status, Status::Ok);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "run waited for the hung renderer"
    );
}

fn outcome(renderer: &str, theme: Theme, status: Status, ms: u64, svg: Option<&str>) -> Outcome {
    Outcome {
        renderer: renderer.into(),
        diagram: "flow".into(),
        theme,
        status,
        elapsed: Duration::from_millis(ms),
        svg: svg.map(Into::into),
    }
}

#[test]
fn results_csv_has_one_row_per_outcome_with_quoted_details() {
    let rows = [
        outcome("echo", Theme::Light, Status::Ok, 12, Some("<svg/>")),
        outcome(
            "refuses",
            Theme::Dark,
            Status::Error("bad \"input\", line 2\nnext".into()),
            3,
            None,
        ),
        outcome("hangs", Theme::Light, Status::Timeout, 5000, None),
    ];
    assert_eq!(
        results_csv(&rows),
        "renderer,diagram,theme,status,ms,svg_bytes,detail\n\
         echo,flow,light,ok,12,6,\n\
         refuses,flow,dark,error,3,0,\"bad \"\"input\"\", line 2 next\"\n\
         hangs,flow,light,timeout,5000,0,\n"
    );
}

#[test]
fn gallery_shows_each_svg_and_each_failure_escaped() {
    let rows = [
        outcome("echo", Theme::Light, Status::Ok, 12, Some("<svg/>")),
        outcome(
            "refuses",
            Theme::Light,
            Status::Error("expected <node> at line 4".into()),
            3,
            None,
        ),
        outcome("hangs", Theme::Light, Status::Timeout, 5000, None),
    ];
    assert_eq!(svg_path(&rows[0]), "out/echo/flow-light.svg");
    assert_eq!(png_path(&rows[0]), "out/echo/flow-light.png");
    let html = gallery_html(&corpus(), &rows);
    // One column per renderer, in run order.
    let cols: Vec<_> = ["<th>echo</th>", "<th>refuses</th>", "<th>hangs</th>"]
        .iter()
        .map(|c| html.find(c).expect(c))
        .collect();
    assert!(cols.windows(2).all(|w| w[0] < w[1]));
    // Shows the resvg raster (what Orbit would draw), linking to the SVG.
    assert!(
        html.contains(r#"<a href="out/echo/flow-light.svg"><img src="out/echo/flow-light.png""#)
    );
    assert!(html.contains("expected &lt;node&gt; at line 4"));
    assert!(html.contains("timeout"));
    // The diagram source is shown, escaped.
    assert!(html.contains("A--&gt;B"));
}

#[test]
fn csv_quotes_any_field_that_needs_it() {
    let mut row = outcome("echo", Theme::Light, Status::Ok, 1, Some("<svg/>"));
    row.diagram = "user-a, \"b\"".into();
    let csv = results_csv(&[row]);
    assert_eq!(
        csv.lines().nth(1),
        Some(r#"echo,"user-a, ""b""",light,ok,1,6,"#)
    );
}
