//! End-to-end smoke tests for the ss-cli binary.

use std::path::PathBuf;
use std::process::Command;

use ss_geom::Shape as _;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ss-cli")).args(args).output().expect("run ss-cli")
}

fn ok(out: &std::process::Output) -> String {
    let s = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "failed: {}", String::from_utf8_lossy(&out.stderr));
    s
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sscli-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

#[test]
fn inspect_convert_send_roundtrip() {
    let project = tmp("heart.ssrs");
    let doc = {
        let mut d = ss_doc::Document::default();
        let l = d.default_layer();
        let heart = ss_geom::kurbo::Circle::new((50.0, 50.0), 20.0).to_path(0.01);
        d.objects.push(ss_doc::Object::path("circle", l, heart));
        d
    };
    std::fs::write(&project, doc.to_json().unwrap()).unwrap();
    let project = project.to_str().unwrap().to_string();

    let out = ok(&cli(&["inspect", &project]));
    assert!(out.contains("page: 304.8 x 304.8 mm"), "{out}");
    assert!(out.contains("objects: 1"), "{out}");

    let out = ok(&cli(&["inspect", "--json", &project]));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["objects"], 1);
    assert_eq!(v["by_cut_action"]["Cut"], 1);

    let svg = tmp("heart.svg").to_str().unwrap().to_string();
    ok(&cli(&["convert", &project, &svg]));
    let back = ss_formats::open(svg.as_ref()).unwrap();
    assert_eq!(back.objects.len(), 1);

    // On-page geometry encodes; a dry run prints stats and sends nothing.
    let out = ok(&cli(&["send", &project, "--dry-run"]));
    assert!(out.contains("1 strokes"), "{out}");

    // Virtual cutter round trip.
    let out = ok(&cli(&["send", &project, "--to", "virtual"]));
    assert!(out.contains("finished"), "{out}");
    assert!(out.contains("virtual cutter: 1 strokes"), "{out}");

    // File transport writes a GPGL stream.
    let gpgl = tmp("job.gpgl");
    let gpgl_s = gpgl.to_str().unwrap().to_string();
    ok(&cli(&["send", &project, "--to", "file", "-o", &gpgl_s]));
    let bytes = std::fs::read(&gpgl).unwrap();
    let sim = ss_gpgl::simulate(&bytes);
    assert_eq!(sim.strokes.len(), 1);
    assert!(sim.errors.is_empty(), "{:?}", sim.errors);

    // Unknown target and a missing file fail cleanly.
    assert!(!cli(&["send", &project, "--to", "nowhere"]).status.success());
    assert!(!cli(&["inspect", "does-not-exist.svg"]).status.success());
}

#[test]
fn craft_batch_text_and_registration() {
    let dir = tmp("batch");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let mut d = ss_doc::Document::default();
    let l = d.default_layer();
    d.objects.push(ss_doc::Object::path("sq", l, ss_geom::Rect::new(40.0, 40.0, 80.0, 70.0).to_path(0.0)));
    d.page.registration = Some(ss_doc::RegMarks::default());
    std::fs::write(dir.join("a.ssrs"), d.to_json().unwrap()).unwrap();
    std::fs::write(dir.join("sub/b.ssrs"), d.to_json().unwrap()).unwrap();
    let out = tmp("batch-out");
    let (dir_s, out_s) = (dir.to_str().unwrap(), out.to_str().unwrap());

    // Only the top folder without --recursive; existing outputs are kept unless --overwrite.
    let o = ok(&cli(&["batch", dir_s, "--to", "craft", "--out", out_s]));
    assert!(o.contains("converted 1, skipped 0"), "{o}");
    let o = ok(&cli(&["batch", dir_s, "--to", "craft", "--out", out_s, "--recursive"]));
    assert!(o.contains("converted 1, skipped 1"), "{o}");
    assert!(out.join("sub/b.craft").is_file());
    let craft = out.join("a.craft");
    let back = ss_formats::open(&craft).unwrap();
    assert_eq!(back.objects.len(), 1);
    assert!(back.page.registration.is_some());

    // The .craft keeps the marks, so the cut can register on them.
    let gpgl = tmp("reg.gpgl");
    let o = ok(&cli(&["send", craft.to_str().unwrap(), "--regmarks", "--auto-search", "--to", "file", "-o", gpgl.to_str().unwrap()]));
    assert!(o.contains("marks found"), "{o}");
    let sim = ss_gpgl::simulate(&std::fs::read(&gpgl).unwrap());
    assert!(sim.registration.is_some_and(|r| r.auto_search));
    assert_eq!(sim.strokes.len(), 1);
    // --auto-search needs --regmarks.
    assert!(!cli(&["send", craft.to_str().unwrap(), "--auto-search", "--dry-run"]).status.success());

    // Text becomes outlines in any output format, when fonts are installed.
    if !ss_text::FontLibrary::system().is_empty() {
        let svg = tmp("text.svg");
        let o = ok(&cli(&["text", "Hi\\nthere", "--size", "15", "-o", svg.to_str().unwrap()]));
        assert!(o.contains(" via "), "{o}");
        let doc = ss_formats::open(&svg).unwrap();
        let b = doc.bounds().unwrap();
        assert!(b.height() > 15.0 && b.width() > 10.0, "{b:?}");
        assert!(!ok(&cli(&["fonts"])).trim().is_empty());
    }
}
