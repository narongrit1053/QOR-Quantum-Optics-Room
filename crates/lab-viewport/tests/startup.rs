//! The viewport's systems must initialise together: a system-parameter conflict (Bevy B0002)
//! panics the first time the schedule runs, which killed the viewport thread at startup.

#[test]
fn viewport_systems_run_headless() {
    lab_viewport::smoke_test(lab_core::Document::new("startup"));
}
