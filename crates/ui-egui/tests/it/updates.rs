//! Help ▸ Check for updates (issue #28), with stand-in release sources (no network).

use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;
use pdfcraft_ui_egui::updates::{Release, UpdateSource, is_newer};

fn source(answer: Result<&str, &str>) -> UpdateSource {
    let answer = answer.map(str::to_string).map_err(str::to_string);
    Arc::new(move || {
        answer.clone().map(|v| Release { installer: None, url: format!("https://github.com/mixlnwuu/maewpdf/releases/tag/{v}"), version: v })
    })
}

fn harness(answer: Result<&str, &str>) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.update_source = Some(source(answer));
        app
    })
}

/// Run frames until the background check has reported. By what the dialog shows, with the
/// same 30-second deadline as `support::wait_until`, not for a fixed second: under load the
/// check's thread can take longer (#1277).
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.run_steps(2);
        if h.query_by_label_contains("Checking for a newer version").is_none() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for the update check");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn versions_compare_by_number() {
    assert!(is_newer("v0.2.0", "0.1.1"));
    assert!(is_newer("v0.1.10", "0.1.9"));
    assert!(is_newer("1", "0.9.9"));
    assert!(!is_newer("v0.1.1", "0.1.1"));
    assert!(!is_newer("v0.1.0", "0.1.1"));
    assert!(!is_newer("v0.1.1-beta.2", "0.1.1"), "suffixes are ignored");
    assert!(!is_newer("nightly", "0.1.1"), "a tag that isn't a version is never newer");
    assert!(!is_newer("v1.2.3.4", "0.1.1"));
    assert!(!is_newer("v99999999999999999999.0.0", "0.1.1"), "out of range");
}

/// The edition's -th.N counts after the numbers (Thai edition).
#[test]
fn thai_edition_releases_compare_by_their_number() {
    assert!(is_newer("v0.10.1-th.2", "0.10.1-th.1"));
    assert!(is_newer("v0.10.1-th.10", "0.10.1-th.9"));
    assert!(is_newer("v0.10.1-th.1", "0.10.1"));
    assert!(is_newer("v0.11.0-th.1", "0.10.1-th.3"));
    assert!(is_newer("v0.10.1-th.2+build.7", "0.10.1-th.1"));
    assert!(!is_newer("v0.10.1-th.1", "0.10.1-th.1"));
    assert!(!is_newer("v0.10.1-th.1", "0.10.1-th.2"));
    assert!(!is_newer("v0.10.1", "0.10.1-th.1"), "the upstream release an edition is built on is not newer");
    assert!(!is_newer("v0.9.0-th.5", "0.10.1-th.1"));
    assert!(!is_newer("v0.10.1-th.x", "0.10.1-th.1"), "a -th. without a number is no version");
}

/// The dialog compares with the edition's own version, -th.N included (Thai edition).
#[test]
fn the_next_thai_edition_release_is_offered_and_this_one_is_up_to_date() {
    let current = pdfcraft_engine::build_info::edition_version();
    let (base, n) = current.rsplit_once("-th.").expect("the edition version has a -th.N");
    let next = format!("v{base}-th.{}", n.parse::<u64>().expect("a number") + 1);
    let mut h = harness(Ok(&next));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains(&format!("PdfCraft {} is available", next.trim_start_matches('v')));
    h.get_by_label_contains(&format!("You have version {current}."));
    h.get_by_label("Download");

    for same in [format!("v{current}"), format!("v{base}")] {
        let mut h = harness(Ok(&same));
        h.state_mut().execute("help.check_updates");
        settle(&mut h);
        h.get_by_label_contains(&format!("PdfCraft {current} is up to date"));
        assert!(h.query_by_label("Download").is_none(), "{same}");
    }
}

#[test]
fn a_newer_release_is_offered_for_download() {
    let mut h = harness(Ok("v99.0.0"));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
    h.get_by_label("Download");
    h.get_by_label("Later").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is available").is_none(), "Later closes the dialog");
}

#[test]
fn an_up_to_date_or_failed_check_says_so() {
    let mut h = harness(Ok(concat!("v", env!("CARGO_PKG_VERSION"))));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("is up to date");
    assert!(h.query_by_label("Download").is_none());

    let mut h = harness(Err("couldn't reach GitHub"));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("Couldn't check for updates: couldn't reach GitHub");
}

#[test]
fn nothing_is_asked_until_the_user_checks() {
    // No check at start (the owner's decision): the source is only called from the command.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        // Settings from a build that had the startup option are ignored.
        app.restore(r#"{"check_updates_at_start": true}"#);
        let counted = counted.clone();
        app.update_source = Some(Arc::new(move || {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Release { version: "v99.0.0".into(), url: "https://github.com/mixlnwuu/maewpdf/releases/tag/v99.0.0".into(), installer: None })
        }));
        app
    });
    settle(&mut h);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(h.query_by_label_contains("is available").is_none());
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    h.get_by_label_contains("PdfCraft 99.0.0 is available");
}

/// A release newer than any build, with an installer.
fn newer_release() -> Release {
    Release {
        version: "v99.0.0".into(),
        url: "https://github.com/mixlnwuu/maewpdf/releases/tag/v99.0.0".into(),
        installer: Some(pdfcraft_ui_egui::updates::Installer {
            url: "https://github.com/mixlnwuu/maewpdf/releases/download/v99.0.0/maewpdf-99.0.0-windows-x64.msi".into(),
            size: 1,
            sha256: "0".repeat(64),
        }),
    }
}

/// Thai edition: an app that checks at start, with `install` standing in for the desktop app's.
fn starting(release: Release, install: bool, runs: Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.update_source = Some(Arc::new(move || Ok(release.clone())));
        app.set_startup_update_check(true);
        if install {
            let run = move |path: &std::path::Path| {
                runs.lock().unwrap().push(path.to_path_buf());
                Ok(())
            };
            app.update_install = Some(pdfcraft_ui_egui::updates::UpdateInstall {
                download: Arc::new(|_, progress, _| {
                    progress(1.0);
                    Ok("update.msi".into())
                }),
                run: Arc::new(run),
            });
        }
        app
    })
}

fn wait_for(h: &mut Harness<'static, PdfCraftApp>, label: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while h.query_by_label(label).is_none() {
        assert!(std::time::Instant::now() < deadline, "{label} never showed");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Thai edition: a newer release at start gets the prompt; Update and restart downloads it and
/// closes, and the installer runs as the app closes.
#[test]
fn a_newer_release_at_start_is_offered_and_installed_on_closing() {
    let runs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut h = starting(newer_release(), true, runs.clone());
    wait_for(&mut h, "ย้ายไป MaewPDF");
    h.get_by_label("What's new");
    h.get_by_label("ย้ายไป MaewPDF").click();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while runs.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "the installer never ran");
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(*runs.lock().unwrap(), [std::path::PathBuf::from("update.msi")]);
}

/// Thai edition: Later puts the prompt away and installs nothing.
#[test]
fn later_leaves_the_app_as_it_is() {
    let runs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut h = starting(newer_release(), true, runs.clone());
    wait_for(&mut h, "ย้ายไป MaewPDF");
    h.get_by_label("Later").click();
    h.run_steps(3);
    assert!(h.query_by_label("ย้ายไป MaewPDF").is_none());
    assert!(runs.lock().unwrap().is_empty());
}

/// Thai edition: without a way to install (a portable copy), the prompt offers the page.
#[test]
fn a_portable_copy_is_offered_the_download_page() {
    let mut h = starting(newer_release(), false, Default::default());
    wait_for(&mut h, "Download");
    assert!(h.query_by_label("ย้ายไป MaewPDF").is_none());
}

/// Thai edition: a check at start that finds nothing newer says nothing, and Preferences turn the
/// check off.
#[test]
fn nothing_new_at_start_says_nothing() {
    let current = Release { version: "v0.0.1".into(), url: "https://github.com/mixlnwuu/maewpdf/releases".into(), installer: None };
    let mut h = starting(current, true, Default::default());
    for _ in 0..30 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.query_by_label("Check for updates").is_none() && h.query_by_label("Later").is_none());

    let mut off = PdfCraftApp::new();
    off.restore(r#"{"check_updates_at_start":false}"#);
    assert!(!off.check_updates_at_start);
    assert!(PdfCraftApp::new().check_updates_at_start, "on unless turned off");
    let mut restored = PdfCraftApp::new();
    restored.restore(&off.persist());
    assert!(!restored.check_updates_at_start, "kept across launches");
}

/// Thai edition: an update that didn't take says so at the next start, once; one that did says
/// nothing. An update that seems to work and leaves the old version is the failure to catch.
#[test]
fn an_update_that_did_not_take_is_said_at_the_next_start() {
    let notified = |wanted: &str| {
        let wanted = wanted.to_owned();
        let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.update_attempt = Some(wanted);
            app.set_startup_update_check(true);
            app.check_updates_at_start = false;
            app
        });
        h.run_steps(3);
        let said = h.query_by_label_contains("wasn't installed").is_some();
        assert_eq!(h.state().update_attempt, None, "checked once");
        said
    };
    assert!(notified("v99.0.0"), "the old version is still here");
    assert!(!notified("v0.0.1"), "this build is newer than what was asked for");
}

/// Bridge release: every start offers MaewPDF, even when today's start already asked, the
/// version was put aside or Preferences turned the check off; the prompt says where the edition
/// went and has no box to put it aside.
#[test]
fn every_start_offers_maewpdf() {
    let mut first = starting(newer_release(), true, Default::default());
    wait_for(&mut first, "ย้ายไป MaewPDF");
    first.get_by_label("PdfCraft ภาษาไทยเปลี่ยนชื่อเป็น MaewPDF แล้ว");
    assert!(first.query_by_label("Don't remind me about this version").is_none());
    let today = first.state().update_asked_on.clone();
    let mut again = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.update_source = Some(Arc::new(|| Ok(newer_release())));
        app.update_install =
            Some(pdfcraft_ui_egui::updates::UpdateInstall { download: Arc::new(|_, _, _| Err("no".into())), run: Arc::new(|_| Ok(())) });
        app.update_asked_on = today;
        app.update_skipped = Some("v99.0.0".into());
        app.check_updates_at_start = false;
        app.set_startup_update_check(true);
        app
    });
    wait_for(&mut again, "ย้ายไป MaewPDF");
}
