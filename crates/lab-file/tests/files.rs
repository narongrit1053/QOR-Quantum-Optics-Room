//! lab-file acceptance tests: atomic writes (A), projects (P), autosave and recovery (S), the
//! worker (W), settings (T) and zip interop (Z8). The zip unit tests (Z1-Z7) live in `zip.rs`
//! because the zip layer is crate-private.

use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use lab_core::glam::DVec2;
use lab_core::sample::demo_document;
use lab_core::{
    Document, ExclusionZone, GridOrigin, HoleGrid, LengthUnit, Opening, OpeningId, Outline, OutlineCorner, Platform,
    PlatformId, PlatformKind, RectCorner, SCHEMA_VERSION, Support, Surface, WallDisplay, ZoneShape,
};
use lab_file::{
    AutosaveSession, EmbeddedFile, FileErrorKind, IoJob, IoResult, IoWorker, PROJECT_EXTENSION, PROJECT_JSON, ProjectFormat,
    ProjectLocation, SaveOptions, Settings, atomic_write, decode_bytes, encode_archive, find_recoverable, load_project,
    load_settings, load_settings_at, save_project, save_settings, save_settings_at, settings_path,
};

fn v1_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../lab-core/tests/fixtures/demo_v1.json")
}

fn archive_at(path: &Path) -> ProjectLocation {
    ProjectLocation { path: path.to_path_buf(), format: ProjectFormat::Archive }
}

fn folder_at(path: &Path) -> ProjectLocation {
    ProjectLocation { path: path.to_path_buf(), format: ProjectFormat::Folder }
}

fn platform(id: u64, name: &str, kind: PlatformKind, outline: Outline, x: f64, grid: HoleGrid) -> Platform {
    Platform {
        id: PlatformId(id),
        name: name.into(),
        kind,
        surface: if kind == PlatformKind::OpticalTable { Surface::Stainless } else { Surface::BlackAnodized },
        outline,
        thickness: 60.0,
        top_height: 800.0,
        position: DVec2::new(x, 0.0),
        rotation_z: 0.3,
        hole_grid: Some(grid),
        support: if kind == PlatformKind::OpticalTable {
            Support::Legs { size: 80.0, inset: 80.0 }
        } else {
            Support::Fixed
        },
    }
}

/// Every outline kind, both grid origins, both zone kinds, a platform without a grid, and
/// openings.
fn rich_document() -> Document {
    let mut doc = Document::new("Rich \u{2014} \u{00e9}\u{4e2d}");
    let zones = vec![
        ExclusionZone {
            name: "Round".into(),
            shape: ZoneShape::Circle { center: DVec2::new(40.0, 20.0), radius: 30.0 },
        },
        ExclusionZone {
            name: "Box".into(),
            shape: ZoneShape::Rect { center: DVec2::new(-100.0, 0.0), size: DVec2::new(60.0, 40.0) },
        },
    ];
    let auto = HoleGrid { exclusions: zones.clone(), ..HoleGrid::metric() };
    let pinned = HoleGrid { origin: GridOrigin::At(DVec2::new(7.5, -2.5)), exclusions: zones, ..HoleGrid::metric() };

    let ids: Vec<u64> = (0..4).map(|_| doc.allocate_id()).collect();
    doc.platforms.push(platform(
        ids[0],
        "Rect",
        PlatformKind::OpticalTable,
        Outline::Rectangle { size: DVec2::new(900.0, 600.0) },
        -1800.0,
        auto,
    ));
    doc.platforms.push(platform(
        ids[1],
        "Rounded",
        PlatformKind::Custom,
        Outline::RoundedRectangle { size: DVec2::new(600.0, 400.0), radius: 50.0 },
        -600.0,
        pinned,
    ));
    doc.platforms.push(platform(
        ids[2],
        "L",
        PlatformKind::Breadboard,
        Outline::LShape {
            size: DVec2::new(600.0, 600.0),
            notch: DVec2::new(250.0, 250.0),
            notch_corner: RectCorner::XPosYPos,
        },
        600.0,
        HoleGrid::metric(),
    ));
    let mut poly = platform(
        ids[3],
        "Polygon",
        PlatformKind::Custom,
        Outline::Polygon {
            corners: vec![
                OutlineCorner::sharp(DVec2::new(-300.0, -200.0)),
                OutlineCorner { point: DVec2::new(300.0, -200.0), radius: 40.0 },
                OutlineCorner::sharp(DVec2::new(300.0, 200.0)),
                OutlineCorner::sharp(DVec2::new(0.0, 60.0)),
                OutlineCorner::sharp(DVec2::new(-300.0, 200.0)),
            ],
        },
        1800.0,
        HoleGrid::metric(),
    );
    poly.hole_grid = None;
    doc.platforms.push(poly);

    let door = OpeningId(doc.allocate_id());
    let window = OpeningId(doc.allocate_id());
    doc.room.openings.push(Opening {
        id: door,
        name: "Door".into(),
        wall: 0,
        offset: 600.0,
        width: 1000.0,
        height: 2100.0,
        sill: 0.0,
    });
    doc.room.openings.push(Opening {
        id: window,
        name: "Window".into(),
        wall: 1,
        offset: 1000.0,
        width: 1200.0,
        height: 1000.0,
        sill: 900.0,
    });
    assert_eq!(doc.validate(), Ok(()), "the test document must be valid");
    doc
}

// ---------------------------------------------------------------- A: atomic_write

#[test]
fn a1_creates_and_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("file.bin");
    atomic_write(&target, b"one").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"one");
    atomic_write(&target, b"second").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"second");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file is left");
}

#[test]
fn a2_replaces_a_target_held_open() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("held.bin");
    fs::write(&target, b"old").unwrap();
    let _held = fs::File::open(&target).unwrap();
    atomic_write(&target, b"new").unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"new");
}

#[test]
fn a3_read_only_target_is_permission_denied() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ro.bin");
    fs::write(&target, b"keep").unwrap();
    let mut perms = fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&target, perms).unwrap();

    let err = atomic_write(&target, b"nope").unwrap_err();
    assert!(matches!(err.kind, FileErrorKind::PermissionDenied), "{err}");
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    let names: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names.len(), 1, "no .tmp left: {names:?}");

    let mut perms = fs::metadata(&target).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&target, perms).unwrap();
}

#[test]
fn a4_missing_parent_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = atomic_write(&dir.path().join("missing").join("f.bin"), b"x").unwrap_err();
    assert!(matches!(err.kind, FileErrorKind::NotFound), "{err}");
    let err = atomic_write(dir.path(), b"x").unwrap_err();
    assert!(matches!(err.kind, FileErrorKind::Invalid(_)), "{err}");
}

// ---------------------------------------------------------------- P: projects

#[test]
fn p1_save_load_save_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    for (n, doc) in [demo_document(), rich_document()].into_iter().enumerate() {
        let first = dir.path().join(format!("a{n}.{PROJECT_EXTENSION}"));
        let second = dir.path().join(format!("b{n}.{PROJECT_EXTENSION}"));
        save_project(&doc, &archive_at(&first), &SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&first).unwrap(), encode_archive(&doc).unwrap());

        let loaded = load_project(&first).unwrap();
        assert!(loaded.document.content_eq(&doc));
        assert_eq!(loaded.document.next_free_id(), doc.next_free_id());
        assert_eq!(loaded.report.migrated_from, None);
        assert_eq!(loaded.location, archive_at(&first));

        save_project(&loaded.document, &archive_at(&second), &SaveOptions::default()).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap(), "document {n}");
    }
}

#[test]
fn p_extras_round_trip_archive_and_folder() {
    let dir = tempfile::tempdir().unwrap();
    let hex = "ab".repeat(32);
    let extras = vec![
        EmbeddedFile { name: "parts/user_X.toml".into(), data: b"id = \"user:X\"".to_vec() },
        EmbeddedFile { name: format!("parts/meshes/{hex}.glb"), data: vec![1, 2, 3] },
        EmbeddedFile { name: "thumbnail.png".into(), data: vec![0x89, b'P', b'N', b'G'] },
    ];
    let options = SaveOptions { extras: extras.clone(), ..Default::default() };
    let sorted = |mut v: Vec<EmbeddedFile>| {
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    };
    let doc = demo_document();
    let archive = archive_at(&dir.path().join(format!("e.{PROJECT_EXTENSION}")));
    let folder = folder_at(&dir.path().join("e"));
    for location in [archive, folder.clone()] {
        save_project(&doc, &location, &options).unwrap();
        let loaded = load_project(&location.path).unwrap();
        assert!(loaded.document.content_eq(&doc));
        assert_eq!(sorted(loaded.extras), sorted(extras.clone()));
    }
    // A folder save without extras deletes the stale ones.
    save_project(&doc, &folder, &SaveOptions::default()).unwrap();
    assert!(load_project(&folder.path).unwrap().extras.is_empty());
}

#[test]
fn p2_folder_loads_from_folder_and_project_json() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("proj");
    let doc = rich_document();
    save_project(&doc, &folder_at(&folder), &SaveOptions::default()).unwrap();
    assert!(folder.join(PROJECT_JSON).is_file());

    for path in [folder.clone(), folder.join(PROJECT_JSON)] {
        let loaded = load_project(&path).unwrap();
        assert!(loaded.document.content_eq(&doc));
        assert_eq!(loaded.location, folder_at(&folder));
    }
    // Saving again over a project folder is fine.
    save_project(&doc, &folder_at(&folder), &SaveOptions::default()).unwrap();
}

#[test]
fn p3_folder_not_empty() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("busy");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("notes.txt"), b"mine").unwrap();
    let err = save_project(&demo_document(), &folder_at(&folder), &SaveOptions::default()).unwrap_err();
    assert!(matches!(err.kind, FileErrorKind::FolderNotEmpty), "{err}");
    assert!(!folder.join(PROJECT_JSON).exists());

    let empty = dir.path().join("empty");
    fs::create_dir(&empty).unwrap();
    save_project(&demo_document(), &folder_at(&empty), &SaveOptions::default()).unwrap();
}

#[test]
fn p4_plain_json_in_a_labsetup_loads() {
    let dir = tempfile::tempdir().unwrap();
    let doc = rich_document();
    let path = dir.path().join("plain.labsetup");
    fs::write(&path, doc.to_json().unwrap()).unwrap();
    assert!(load_project(&path).unwrap().document.content_eq(&doc));

    let mut with_bom = b"\xEF\xBB\xBF".to_vec();
    with_bom.extend_from_slice(doc.to_json().unwrap().as_bytes());
    fs::write(&path, with_bom).unwrap();
    assert!(load_project(&path).unwrap().document.content_eq(&doc));
}

#[test]
fn p5_newer_schema_in_a_zip() {
    let mut doc = demo_document();
    doc.schema_version = SCHEMA_VERSION + 1;
    let bytes = encode_archive(&doc).unwrap();
    assert!(bytes.starts_with(b"PK\x03\x04"));
    match decode_bytes(&bytes) {
        Err(FileErrorKind::NewerSchema { found, supported }) => {
            assert_eq!((found, supported), (SCHEMA_VERSION + 1, SCHEMA_VERSION));
        }
        other => panic!("expected NewerSchema, got {:?}", other.map(|_| ())),
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.labsetup");
    fs::write(&path, &bytes).unwrap();
    let err = load_project(&path).expect_err("must fail");
    assert!(matches!(err.kind, FileErrorKind::NewerSchema { .. }), "{err}");
}

#[test]
fn p6_backup_is_created_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.labsetup");
    let original = fs::read(v1_fixture()).unwrap();
    fs::write(&path, &original).unwrap();
    let backup = dir.path().join("old.labsetup.v1.bak");
    let options = SaveOptions { backup_schema: Some(1), ..Default::default() };

    let loaded = load_project(&path).unwrap();
    save_project(&loaded.document, &archive_at(&path), &options).unwrap();
    assert_eq!(fs::read(&backup).unwrap(), original);
    assert!(fs::read(&path).unwrap().starts_with(b"PK"), "the target is now an archive");

    let mut changed = loaded.document.clone();
    changed.name = "Changed".into();
    save_project(&changed, &archive_at(&path), &options).unwrap();
    assert_eq!(fs::read(&backup).unwrap(), original, "an existing backup is not overwritten");
    assert_eq!(load_project(&path).unwrap().document.name, "Changed");
}

#[test]
fn p7_v1_fixture_migrates_on_load() {
    let loaded = load_project(&v1_fixture()).unwrap();
    assert_eq!(loaded.report.migrated_from, Some(1));
    assert_eq!(loaded.document.schema_version, SCHEMA_VERSION);
    assert_eq!(loaded.location.format, ProjectFormat::Archive);
    assert_eq!(loaded.document.validate(), Ok(()));
}

#[test]
fn load_errors_are_classified() {
    let dir = tempfile::tempdir().unwrap();
    let kind = |path: &Path| load_project(path).expect_err("must fail").kind;

    assert!(matches!(kind(&dir.path().join("missing.labsetup")), FileErrorKind::NotFound));

    let junk = dir.path().join("junk.labsetup");
    fs::write(&junk, b"just some text").unwrap();
    assert!(matches!(kind(&junk), FileErrorKind::NotAProject(_)));

    let empty = dir.path().join("empty.labsetup");
    fs::write(&empty, b"").unwrap();
    assert!(matches!(kind(&empty), FileErrorKind::NotAProject(_)));

    let truncated = dir.path().join("cut.labsetup");
    let bytes = encode_archive(&demo_document()).unwrap();
    fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
    assert!(matches!(kind(&truncated), FileErrorKind::Corrupt(_)));

    let broken_json = dir.path().join("broken.labsetup");
    fs::write(&broken_json, b"{ \"schema_version\": 2, ").unwrap();
    assert!(matches!(kind(&broken_json), FileErrorKind::Corrupt(_)));

    let folder = dir.path().join("nothing-here");
    fs::create_dir(&folder).unwrap();
    assert!(matches!(kind(&folder), FileErrorKind::NotAProject(_)));
}

#[test]
fn display_name_uses_stem_or_folder_name() {
    assert_eq!(archive_at(Path::new("C:/labs/Rb setup.labsetup")).display_name(), "Rb setup");
    assert_eq!(folder_at(Path::new("C:/labs/Rb folder")).display_name(), "Rb folder");
}

#[test]
fn saving_an_invalid_document_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.labsetup");
    let mut doc = demo_document();
    doc.platforms[0].thickness = f64::NAN;
    let err = save_project(&doc, &archive_at(&path), &SaveOptions::default()).unwrap_err();
    assert!(matches!(err.kind, FileErrorKind::Internal(_)), "{err}");
    assert!(!path.exists());
}

// ---------------------------------------------------------------- S: autosave and recovery

/// The child half of S1: starts a session, autosaves and exits without any cleanup.
#[test]
#[ignore = "helper process for s1_crashed_session_is_recovered"]
fn helper_crash_session() {
    let Some(root) = std::env::var_os("LABSETUP_TEST_CRASH_ROOT") else {
        return;
    };
    let mut session = AutosaveSession::start(Path::new(&root), "crashed").unwrap();
    session.write(&demo_document(), "crashed", None).unwrap();
    std::process::exit(3);
}

#[test]
fn s1_crashed_session_is_recovered() {
    let root = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "helper_crash_session", "--test-threads=1"])
        .env("LABSETUP_TEST_CRASH_ROOT", root.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(3), "the helper must exit without cleanup");

    let mut found = find_recoverable(root.path(), None);
    assert_eq!(found.len(), 1);
    let candidate = found.pop().unwrap();
    let info = candidate.info.as_ref().expect("session.json was written");
    assert_eq!(info.title, "crashed");
    assert!(info.autosaved_at_ms.is_some());
    assert!(!candidate.newer_format());

    let loaded = candidate.load().unwrap();
    assert!(loaded.document.content_eq(&demo_document()));

    assert!(find_recoverable(root.path(), None).is_empty(), "a held candidate is not offered twice");

    let dir = candidate.dir.clone();
    candidate.discard().unwrap();
    assert!(!dir.exists());
}

#[test]
fn s2_live_session_is_skipped() {
    let root = tempfile::tempdir().unwrap();
    let mut session = AutosaveSession::start(root.path(), "live").unwrap();
    session.write(&demo_document(), "live", None).unwrap();
    assert!(find_recoverable(root.path(), None).is_empty());
    assert!(find_recoverable(root.path(), Some(session.dir())).is_empty());
    let dir = session.dir().to_path_buf();
    assert!(dir.join("autosave.labsetup").is_file());

    session.end().unwrap();
    assert!(!dir.exists(), "a clean exit removes the session");
}

#[test]
fn s3_dead_session_without_autosave_is_cleaned() {
    let root = tempfile::tempdir().unwrap();
    let dead = root.path().join("s-1-1");
    fs::create_dir(&dead).unwrap();
    fs::write(dead.join("lock"), b"").unwrap();
    fs::write(dead.join("session.json"), b"{}").unwrap();
    let no_lock = root.path().join("s-2-2");
    fs::create_dir(&no_lock).unwrap();
    fs::write(no_lock.join("leftover.tmp"), b"x").unwrap();

    assert!(find_recoverable(root.path(), None).is_empty());
    assert!(!dead.exists());
    assert!(!no_lock.exists());
}

#[test]
fn s3b_dead_session_with_autosave_keeps_it_and_drops_temp_files() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("s-5-5");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("autosave.labsetup"), encode_archive(&rich_document()).unwrap()).unwrap();
    fs::write(dir.join("session.json"), b"not json").unwrap();
    fs::write(dir.join(".autosave.labsetup.9.9.tmp"), b"half").unwrap();

    let mut found = find_recoverable(root.path(), None);
    assert_eq!(found.len(), 1);
    let candidate = found.pop().unwrap();
    assert!(candidate.info.is_none(), "an unreadable session.json gives no info");
    assert!(!dir.join(".autosave.labsetup.9.9.tmp").exists());
    assert!(candidate.load().unwrap().document.content_eq(&rich_document()));
    candidate.release();
    assert!(dir.exists(), "release keeps the files");
    assert_eq!(find_recoverable(root.path(), None).len(), 1, "and offers them again later");
}

#[test]
fn s3c_newer_format_autosave_is_flagged_and_kept() {
    let root = tempfile::tempdir().unwrap();
    let mut session = AutosaveSession::start(root.path(), "future").unwrap();
    session.write(&demo_document(), "future", None).unwrap();
    let dir = session.dir().to_path_buf();
    // Simulate another process: a crashed session written by a newer version.
    drop(session);
    let mut info: serde_json::Value = serde_json::from_slice(&fs::read(dir.join("session.json")).unwrap()).unwrap();
    info["schema_version"] = serde_json::json!(SCHEMA_VERSION + 1);
    fs::write(dir.join("session.json"), serde_json::to_vec(&info).unwrap()).unwrap();

    let found = find_recoverable(root.path(), None);
    assert_eq!(found.len(), 1);
    assert!(found[0].newer_format());
    found.into_iter().next().unwrap().release();
    assert!(dir.exists());
}

#[test]
fn s3d_write_clear_and_location_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let mut session = AutosaveSession::start(root.path(), "t").unwrap();
    let where_ = archive_at(Path::new("C:/labs/setup.labsetup"));
    session.write(&demo_document(), "setup", Some(&where_)).unwrap();
    let dir = session.dir().to_path_buf();
    drop(session);
    let candidate = find_recoverable(root.path(), None).pop().unwrap();
    assert_eq!(candidate.info.as_ref().unwrap().location.as_ref(), Some(&where_));
    assert_eq!(candidate.load().unwrap().location, where_);
    candidate.release();

    // A session that autosaved and then saved (clear) leaves nothing to recover.
    fs::remove_dir_all(&dir).unwrap();
    let mut session = AutosaveSession::start(root.path(), "t").unwrap();
    session.write(&demo_document(), "t", None).unwrap();
    session.clear().unwrap();
    session.clear().unwrap();
    assert!(!session.dir().join("autosave.labsetup").exists());
    drop(session);
    assert!(find_recoverable(root.path(), None).is_empty());
}

#[test]
fn s4_a_panic_does_not_delete_the_session() {
    let root = tempfile::tempdir().unwrap();
    let mut session = AutosaveSession::start(root.path(), "panicky").unwrap();
    session.write(&demo_document(), "panicky", None).unwrap();
    let dir = session.dir().to_path_buf();

    let result = panic::catch_unwind(AssertUnwindSafe(move || {
        let _session = session;
        panic!("boom");
    }));
    assert!(result.is_err());
    assert!(dir.join("autosave.labsetup").is_file(), "the autosave survives the unwind");
    assert_eq!(find_recoverable(root.path(), None).len(), 1);
}

#[test]
fn default_root_and_settings_path_follow_the_environment() {
    // Only this test touches these variables.
    let dir = tempfile::tempdir().unwrap();
    let autosave = dir.path().join("auto");
    let settings = dir.path().join("cfg").join("settings.json");
    // SAFETY: no other test in this binary reads or writes these two variables.
    unsafe {
        std::env::set_var("LABSETUP_AUTOSAVE_DIR", &autosave);
        std::env::set_var("LABSETUP_SETTINGS", &settings);
    }
    assert_eq!(lab_file::default_root(), autosave);
    assert_eq!(settings_path(), settings);

    // T1 (env flavour): a corrupt settings file is renamed and defaults are returned.
    assert_eq!(load_settings(), Settings::default());
    let wanted = Settings {
        display_unit: LengthUnit::Inch,
        wall_display: WallDisplay::Always,
        last_dir: Some(dir.path().into()),
        ..Default::default()
    };
    save_settings(&wanted).unwrap();
    assert_eq!(load_settings(), wanted);
    fs::write(&settings, b"{ not json").unwrap();
    assert_eq!(load_settings(), Settings::default());
    assert!(!settings.exists());
    assert_eq!(fs::read(settings.with_extension("json.corrupt")).unwrap(), b"{ not json");
}

// ---------------------------------------------------------------- W: worker, T: settings

fn collect_results() -> (impl Fn(IoResult) + Send + 'static, mpsc::Receiver<IoResult>) {
    let (tx, rx) = mpsc::channel();
    (move |result| drop(tx.send(result)), rx)
}

#[test]
fn w1_jobs_run_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("w.labsetup");
    let doc = rich_document();
    let (on_result, rx) = collect_results();
    let session = AutosaveSession::start(&dir.path().join("auto"), "w").unwrap();
    let session_dir = session.dir().to_path_buf();
    let worker = IoWorker::start(Some(session), on_result);

    worker.submit(IoJob::Save {
        ticket: 1,
        doc: Box::new(doc.clone()),
        location: archive_at(&path),
        options: SaveOptions::default(),
    });
    worker.submit(IoJob::Load { ticket: 2, path: path.clone() });
    worker.submit(IoJob::Load { ticket: 3, path: dir.path().join("missing.labsetup") });
    worker.submit(IoJob::Autosave { ticket: 4, doc: Box::new(doc.clone()), title: "w".into(), location: None });
    worker.submit(IoJob::ClearAutosave);
    worker.submit(IoJob::Autosave { ticket: 5, doc: Box::new(doc.clone()), title: "w".into(), location: None });
    worker.submit(IoJob::EndSession);
    worker.submit(IoJob::Autosave { ticket: 6, doc: Box::new(doc.clone()), title: "w".into(), location: None });
    assert!(worker.shutdown(Duration::from_secs(20)));

    let results: Vec<IoResult> = rx.try_iter().collect();
    assert_eq!(results.len(), 6, "{results:?}");
    match &results[0] {
        IoResult::Saved { ticket: 1, location, result: Ok(()) } => assert_eq!(location, &archive_at(&path)),
        other => panic!("{other:?}"),
    }
    match &results[1] {
        IoResult::Loaded { ticket: 2, result: Ok(loaded) } => assert!(loaded.document.content_eq(&doc)),
        other => panic!("{other:?}"),
    }
    match &results[2] {
        IoResult::Loaded { ticket: 3, result: Err(e) } => assert!(matches!(e.kind, FileErrorKind::NotFound)),
        other => panic!("{other:?}"),
    }
    assert!(matches!(&results[3], IoResult::Autosaved { ticket: 4, result: Ok(()) }));
    assert!(matches!(&results[4], IoResult::Autosaved { ticket: 5, result: Ok(()) }));
    match &results[5] {
        IoResult::Autosaved { ticket: 6, result: Err(e) } => assert!(matches!(e.kind, FileErrorKind::Internal(_))),
        other => panic!("{other:?}"),
    }
    assert!(!session_dir.exists(), "EndSession removed the session");
}

#[test]
fn w1_without_a_session_autosave_is_unavailable() {
    let (on_result, rx) = collect_results();
    let worker = IoWorker::start(None, on_result);
    worker.submit(IoJob::Autosave { ticket: 1, doc: Box::new(demo_document()), title: "x".into(), location: None });
    worker.submit(IoJob::ClearAutosave);
    worker.submit(IoJob::EndSession);
    assert!(worker.shutdown(Duration::from_secs(10)));
    let results: Vec<IoResult> = rx.try_iter().collect();
    assert_eq!(results.len(), 1);
    assert!(
        matches!(&results[0], IoResult::Autosaved { ticket: 1, result: Err(e) } if matches!(e.kind, FileErrorKind::Internal(_)))
    );
}

#[test]
fn t1_corrupt_settings_are_renamed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("settings.json");
    assert_eq!(load_settings_at(&path), Settings::default());

    let wanted = Settings {
        display_unit: LengthUnit::Inch,
        wall_display: WallDisplay::Hidden,
        last_dir: Some(PathBuf::from("C:/labs")),
        ..Default::default()
    };
    save_settings_at(&path, &wanted).unwrap();
    assert_eq!(load_settings_at(&path), wanted);

    fs::write(&path, b"\x00\x01 garbage").unwrap();
    assert_eq!(load_settings_at(&path), Settings::default());
    assert!(!path.exists());
    assert!(dir.path().join("sub").join("settings.json.corrupt").is_file());

    // Missing fields fall back to defaults.
    fs::write(&path, br#"{"display_unit":"Inch"}"#).unwrap();
    let partial = load_settings_at(&path);
    assert_eq!(partial.display_unit, LengthUnit::Inch);
    assert_eq!(partial.wall_display, WallDisplay::default());
}

// ---------------------------------------------------------------- Z8: zip interop (manual)

#[test]
#[ignore = "run once manually in T7: needs Windows PowerShell"]
fn z8_powershell_zip_interop() {
    let dir = tempfile::tempdir().unwrap();
    let doc = rich_document();
    let ours = dir.path().join("ours.zip");
    save_project(&doc, &archive_at(&ours), &SaveOptions::default()).unwrap();

    let out = dir.path().join("extracted");
    let ps = |script: String| {
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()
            .unwrap();
        assert!(status.success(), "PowerShell failed: {script}");
    };
    ps(format!("Expand-Archive -LiteralPath '{}' -DestinationPath '{}'", ours.display(), out.display()));
    assert_eq!(fs::read_to_string(out.join(PROJECT_JSON)).unwrap(), doc.to_json().unwrap());

    let theirs = dir.path().join("theirs.labsetup.zip");
    ps(format!(
        "Compress-Archive -LiteralPath '{}' -DestinationPath '{}'",
        out.join(PROJECT_JSON).display(),
        theirs.display()
    ));
    let loaded = load_project(&theirs).unwrap();
    assert!(loaded.document.content_eq(&doc));
}
