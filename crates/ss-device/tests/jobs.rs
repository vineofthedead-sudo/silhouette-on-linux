//! End-to-end job runs against the virtual cutter and the file transport.

use std::time::Duration;

use ss_device::{FileTransport, JobEvent, RunOptions, VirtualCutter, spawn_job};
use ss_geom::{Rect, Shape};
use ss_gpgl::{JobSettings, encode};

fn job() -> Vec<u8> {
    let paths = [Rect::new(10.0, 10.0, 60.0, 40.0).to_path(0.0), Rect::new(20.0, 20.0, 30.0, 30.0).to_path(0.0)];
    encode(&paths, &JobSettings::default()).unwrap().bytes
}

fn fast() -> RunOptions {
    RunOptions {
        ready_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(2),
        poll_interval: Duration::from_millis(5),
        ..RunOptions::default()
    }
}

#[test]
fn full_job_on_virtual_cutter() {
    let (vc, h) = VirtualCutter::new();
    h.set_busy_polls(0);
    let bytes = job();
    let events = spawn_job(vc, bytes.clone(), fast()).wait();
    assert_eq!(
        events.first(),
        Some(&JobEvent::Connected { device: "Virtual cutter".into(), firmware: Some("VIRTUAL 1.0".into()) })
    );
    assert!(events.contains(&JobEvent::Sending { sent: bytes.len(), total: bytes.len() }));
    assert_eq!(&events[events.len() - 2..], &[JobEvent::Finishing, JobEvent::Finished]);
    assert_eq!(h.received(), bytes, "the cutter must receive exactly the job");
    assert_eq!(h.initialized(), 1);
    let sim = h.simulation();
    assert_eq!(sim.strokes.len(), 2);
    assert!(sim.errors.is_empty() && sim.homed);
}

#[test]
fn waits_for_media_then_cuts() {
    let (vc, h) = VirtualCutter::new();
    h.set_unloaded(true);
    let handle = spawn_job(vc, job(), fast());
    let ev = handle.events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(ev, JobEvent::Connected { .. }));
    assert_eq!(handle.events.recv_timeout(Duration::from_secs(5)).unwrap(), JobEvent::WaitingForMedia);
    assert!(h.received().is_empty(), "nothing is sent before media is loaded");
    h.set_unloaded(false);
    let events = handle.wait();
    assert_eq!(events.last(), Some(&JobEvent::Finished));
    assert_eq!(h.received(), job());
}

#[test]
fn partial_writes_and_busy_cutter() {
    let (vc, h) = VirtualCutter::new();
    h.set_accept_limit(Some(7));
    let handle = spawn_job(vc, job(), fast());
    // Report "moving" for a while once data starts flowing, as a real cutter does.
    h.set_busy_polls(5);
    let events = handle.wait();
    assert_eq!(events.last(), Some(&JobEvent::Finished));
    assert_eq!(h.received(), job());
    let sending: Vec<_> = events.iter().filter(|e| matches!(e, JobEvent::Sending { .. })).collect();
    assert!(sending.len() > 10, "progress is reported as data goes out");
}

#[test]
fn stalled_cutter_times_out() {
    let (vc, h) = VirtualCutter::new();
    h.set_stalled(true);
    let opts = RunOptions { stall_timeout: Duration::from_millis(100), ..fast() };
    let events = spawn_job(vc, job(), opts).wait();
    assert!(matches!(events.last(), Some(JobEvent::Failed(m)) if m.contains("did not respond")), "{events:?}");
}

#[test]
fn cancel_stops_sending() {
    let (vc, h) = VirtualCutter::new();
    h.set_stalled(true);
    let opts = RunOptions { stall_timeout: Duration::from_secs(30), ..fast() };
    let handle = spawn_job(vc, job(), opts);
    std::thread::sleep(Duration::from_millis(50));
    handle.cancel();
    let events = handle.wait();
    assert_eq!(events.last(), Some(&JobEvent::Cancelled));
    assert!(h.received().is_empty());
}

fn registration_job() -> ss_gpgl::Job {
    let page = ss_doc::Page::preset("letter").unwrap();
    let reg = ss_gpgl::Registration::from_marks(&ss_doc::RegMarks::default(), &page, true);
    let settings = JobSettings { media_mm: (page.width_mm, page.height_mm), registration: Some(reg), ..JobSettings::default() };
    encode(&[Rect::new(30.0, 40.0, 60.0, 70.0).to_path(0.0)], &settings).unwrap()
}

#[test]
fn registration_pauses_until_marks_are_found() {
    let (vc, h) = VirtualCutter::new();
    let job = registration_job();
    let events = ss_device::spawn_cut_job(vc, job.clone(), fast()).wait();
    let pos = |e: &JobEvent| events.iter().position(|x| x == e).unwrap_or_else(|| panic!("{e:?} missing from {events:?}"));
    let end = job.registration_end.unwrap();
    // The search is sent in full, then the cutter is heard from, then the cut follows.
    assert!(pos(&JobEvent::Sending { sent: end, total: job.bytes.len() }) < pos(&JobEvent::Registering));
    assert!(pos(&JobEvent::Registering) < pos(&JobEvent::Registered));
    assert!(pos(&JobEvent::Registered) < pos(&JobEvent::Sending { sent: job.bytes.len(), total: job.bytes.len() }));
    assert_eq!(events.last(), Some(&JobEvent::Finished));
    assert_eq!(h.received(), job.bytes);
    assert!(h.simulation().registration.is_some_and(|r| r.auto_search));
}

#[test]
fn registration_failure_stops_before_cutting() {
    let job = registration_job();
    let end = job.registration_end.unwrap();
    let (vc, h) = VirtualCutter::new();
    h.set_registration_reply(None);
    let opts = RunOptions { registration_timeout: Duration::from_millis(200), ..fast() };
    let events = ss_device::spawn_cut_job(vc, job.clone(), opts).wait();
    assert!(matches!(events.last(), Some(JobEvent::Failed(m)) if m.contains("registration marks not found")), "{events:?}");
    assert_eq!(h.received(), job.bytes[..end], "nothing past the mark search may be sent");

    let (vc, h) = VirtualCutter::new();
    h.set_registration_reply(Some(b"    1\x03".to_vec()));
    let events = ss_device::spawn_cut_job(vc, job.clone(), fast()).wait();
    assert!(matches!(events.last(), Some(JobEvent::Failed(m)) if m.contains("\"1\"")), "{events:?}");
    assert_eq!(h.received().len(), end);
}

#[test]
fn file_transport_writes_only_the_job() {
    let path = std::env::temp_dir().join(format!("ss-device-test-{}.gpgl", std::process::id()));
    let t = FileTransport::create(&path).unwrap();
    let events = spawn_job(t, job(), fast()).wait();
    assert_eq!(events.last(), Some(&JobEvent::Finished));
    assert_eq!(std::fs::read(&path).unwrap(), job());
    std::fs::remove_file(path).unwrap();
}
