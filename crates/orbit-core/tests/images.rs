//! Image links in a note read Attachments inside the vault.

use std::fs;

use orbit_core::{Vault, VaultPath};

fn p(path: &str) -> VaultPath {
    VaultPath::new(path).unwrap()
}

#[test]
fn image_links_read_attachments_relative_to_the_note_and_never_leave_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("Projects/assets")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::write(root.join("Projects/assets/My chart.png"), "png").unwrap();
    fs::write(root.join("shared/logo.svg"), "svg").unwrap();
    let vault = Vault::open(root).unwrap();
    let note = p("Projects/plan.md");
    let read = |link: &str| vault.read_linked_attachment(&note, link);
    let bytes = |s: &str| Some(s.as_bytes().to_vec());

    assert_eq!(read("assets/My%20chart.png"), bytes("png"));
    assert_eq!(read("./assets/My chart.png"), bytes("png"));
    assert_eq!(read("../shared/logo.svg#dark"), bytes("svg"));
    // Outside the vault, missing, absolute or remote: not a vault file.
    assert_eq!(read("../../etc/passwd"), None);
    assert_eq!(read("assets/missing.png"), None);
    assert_eq!(read("/etc/hostname"), None);
    assert_eq!(read("https://example.com/a.png"), None);
}
