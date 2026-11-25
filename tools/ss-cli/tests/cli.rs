//! End-to-end smoke tests for the ss-cli binary.

use std::path::PathBuf;
use std::process::Command;

use ss_geom::kurbo::Shape as _;

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
