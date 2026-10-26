//! App settings, loaded from and saved to a config folder.

use std::fs;

use orbit_core::{Settings, ThemeChoice};

#[test]
fn missing_settings_mean_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let s = Settings::load(dir.path());
    assert_eq!(s.theme, ThemeChoice::OrbitPaper);
    assert!(s.paper_grain);
    assert_eq!(s.interface_size_percent(), 100);
}

#[test]
fn saved_settings_load_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Settings::default();
    s.theme = ThemeChoice::MatchSystem;
    s.paper_grain = false;
    s.grow_interface();
    s.grow_interface();
    s.save(dir.path()).unwrap();
    assert_eq!(Settings::load(dir.path()), s);
}

#[test]
fn a_corrupt_settings_file_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
    assert_eq!(Settings::load(dir.path()), Settings::default());
}

#[test]
fn unknown_keys_are_ignored_and_known_ones_kept() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("settings.json"),
        r#"{ "theme": "orbit-paper-dark", "from_the_future": 42 }"#,
    )
    .unwrap();
    let s = Settings::load(dir.path());
    assert_eq!(s.theme, ThemeChoice::OrbitPaperDark);
    assert!(s.paper_grain, "missing keys take their defaults");
}

fn load_json(json: &str) -> Settings {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("settings.json"), json).unwrap();
    Settings::load(dir.path())
}

#[test]
fn hand_edited_interface_sizes_are_clamped_and_stepped_on_load() {
    assert_eq!(
        load_json(r#"{ "interface_size_percent": 999 }"#).interface_size_percent(),
        150
    );
    assert_eq!(
        load_json(r#"{ "interface_size_percent": 10 }"#).interface_size_percent(),
        80
    );
    assert_eq!(
        load_json(r#"{ "interface_size_percent": 114 }"#).interface_size_percent(),
        110
    );
}

#[test]
fn one_bad_value_keeps_the_others() {
    let s =
        load_json(r#"{ "theme": "sepia", "paper_grain": false, "interface_size_percent": "big" }"#);
    assert_eq!(s.theme, ThemeChoice::OrbitPaper, "unknown theme falls back");
    assert!(!s.paper_grain, "valid field survives");
    assert_eq!(s.interface_size_percent(), 100);
}

#[test]
fn the_interface_grows_and_shrinks_in_steps_within_its_range() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Settings::load(dir.path());
    assert!(s.can_shrink_interface() && s.can_grow_interface());
    s.grow_interface();
    s.save(dir.path()).unwrap();
    let mut s = Settings::load(dir.path());
    assert_eq!(s.interface_size_percent(), 110);
    for _ in 0..20 {
        s.grow_interface();
    }
    assert_eq!(s.interface_size_percent(), 150);
    assert!(!s.can_grow_interface());
    for _ in 0..20 {
        s.shrink_interface();
    }
    assert_eq!(s.interface_size_percent(), 80);
    assert!(!s.can_shrink_interface());
}
