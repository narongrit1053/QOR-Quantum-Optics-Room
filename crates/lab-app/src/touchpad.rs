//! Three-finger touchpad slide orbits the view (POLA-157). Windows keeps three-finger gestures
//! for itself and sends apps nothing, so this reads the precision touchpad's raw HID reports
//! (page 0x0D, usage 0x05) and follows the fingers itself. The user must set Windows'
//! three-finger swipes to Nothing, or Windows switches apps at the same time.

use std::cell::RefCell;
use std::rc::Rc;

use lab_ui::AppWindow;
use lab_viewport::Vec2;

use crate::app_state::AppState;

/// Viewport px per normalised pad width (0..1).
#[cfg_attr(not(windows), allow(dead_code))]
const PX_PER_PAD: f32 = 800.0;

/// Hooks the main window's touchpad input. Call once at startup.
pub fn install(ui: &AppWindow, app: &Rc<RefCell<AppState>>) {
    #[cfg(windows)]
    win::install(slint::ComponentHandle::as_weak(ui), app.clone(), 0);
    #[cfg(not(windows))]
    let _ = (ui, app);
}

/// One touchpad frame: returns the centroid of exactly three tips (`None` otherwise) and its move
/// since `prev`, the previous frame's centroid. Any other finger count resets the gesture.
#[cfg_attr(not(windows), allow(dead_code))]
fn step(prev: Option<Vec2>, tips: &[Vec2]) -> (Option<Vec2>, Option<Vec2>) {
    if tips.len() != 3 {
        return (None, None);
    }
    let centroid = tips.iter().copied().sum::<Vec2>() / 3.0;
    (Some(centroid), prev.map(|p| centroid - p))
}

#[cfg(windows)]
mod win {
    use std::collections::HashMap;
    use std::mem::{offset_of, size_of, zeroed};
    use std::ptr::null_mut;
    use std::time::Duration;

    use lab_viewport::{ViewportInput, ViewportRequest};
    use slint::ComponentHandle;
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDP_CAPS, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS, HidP_GetCaps, HidP_GetUsageValue, HidP_GetUsages, HidP_GetValueCaps,
        HidP_Input, PHIDP_PREPARSED_DATA,
    };
    use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Input::{
        GetRawInputData, GetRawInputDeviceInfoW, HRAWINPUT, RAWHID, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
        RIDI_PREPARSEDDATA, RIM_TYPEHID, RegisterRawInputDevices,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_INPUT;

    use super::*;

    /// How long after the last touchpad report `trackpad_active` stays true.
    const ACTIVE_FOR: Duration = Duration::from_millis(300);

    pub(super) fn install(ui: slint::Weak<AppWindow>, app: Rc<RefCell<AppState>>, tries: u32) {
        // The window gets its HWND only once the event loop shows it.
        slint::Timer::single_shot(Duration::from_millis(if tries == 0 { 0 } else { 200 }), move || {
            let Some(window) = ui.upgrade() else { return };
            let hwnd = crate::dialogs::owner_hwnd(window.window()) as HWND;
            if hwnd.is_null() {
                if tries < 20 {
                    install(ui, app, tries + 1);
                }
                return;
            }
            let idle = slint::Timer::default();
            let a = app.clone();
            idle.start(slint::TimerMode::SingleShot, ACTIVE_FOR, move || {
                if let Ok(mut a) = a.try_borrow_mut() {
                    a.trackpad_active = false;
                }
            });
            let state = Box::into_raw(Box::new(State { app, devices: HashMap::new(), frame: Frame::default(), idle }));
            // Foreground only (no RIDEV_INPUTSINK). winit registers mouse and keyboard, a different usage.
            let device = RAWINPUTDEVICE { usUsagePage: 0x0D, usUsage: 0x05, dwFlags: 0, hwndTarget: hwnd };
            // SAFETY: `hwnd` is this thread's live window; `state` is leaked on success and lives as
            // long as the window (the subclass proc is its only user, on this thread).
            unsafe {
                if RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) == 0
                    || SetWindowSubclass(hwnd, Some(subclass_proc), 0x7043_5450, state as usize) == 0
                {
                    eprintln!("touchpad: raw input unavailable; three-finger orbit is off");
                    drop(Box::from_raw(state));
                }
            }
        });
    }

    unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, data: usize) -> LRESULT {
        if msg == WM_INPUT {
            // SAFETY: `data` is the `State` leaked by `install`; window procs run on the UI thread
            // only, and nothing below pumps messages, so this is the only reference.
            let state = unsafe { &mut *(data as *mut State) };
            // A malformed report is ignored; a panic must never unwind into Windows.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.input(lparam as HRAWINPUT)));
        }
        // Always pass it on: winit reads its own WM_INPUT for the mouse.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    struct State {
        app: Rc<RefCell<AppState>>,
        /// Per raw input device; `None` when it is not a usable touchpad.
        devices: HashMap<HANDLE, Option<Device>>,
        frame: Frame,
        /// Clears `trackpad_active` `ACTIVE_FOR` after the last report.
        idle: slint::Timer,
    }

    /// A touchpad frame being collected: a hybrid-mode pad splits one frame over several reports,
    /// with the contact count in the first one only.
    #[derive(Default)]
    struct Frame {
        tips: Vec<Vec2>,
        remaining: u32,
        prev: Option<Vec2>,
    }

    struct Device {
        /// The preparsed data, as u64s for alignment.
        preparsed: Vec<u64>,
        /// Each finger's link collection with its logical X and Y ranges.
        fingers: Vec<(u16, (i32, i32), (i32, i32))>,
    }

    impl State {
        fn input(&mut self, handle: HRAWINPUT) {
            let header = size_of::<RAWINPUTHEADER>() as u32;
            let mut size = 0u32;
            // SAFETY: a size query with a null buffer.
            if unsafe { GetRawInputData(handle, RID_INPUT, null_mut(), &mut size, header) } != 0 || (size as usize) < size_of::<RAWINPUT>() {
                return;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            // SAFETY: `buf` holds `size` bytes, aligned for RAWINPUT.
            if unsafe { GetRawInputData(handle, RID_INPUT, buf.as_mut_ptr().cast(), &mut size, header) } != size {
                return;
            }
            // SAFETY: GetRawInputData filled at least a RAWINPUT; the union read is checked by dwType.
            let (device, hid_size, count) = unsafe {
                let raw = &*(buf.as_ptr() as *const RAWINPUT);
                if raw.header.dwType != RIM_TYPEHID {
                    return;
                }
                (raw.header.hDevice, raw.data.hid.dwSizeHid as usize, raw.data.hid.dwCount as usize)
            };
            let start = offset_of!(RAWINPUT, data) + offset_of!(RAWHID, bRawData);
            // SAFETY: `buf` is `size` initialised bytes.
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), size as usize) };
            let Some(reports) = hid_size.checked_mul(count).and_then(|n| bytes.get(start..start + n)) else { return };
            if hid_size == 0 {
                return;
            }
            let Some(device) = self.devices.entry(device).or_insert_with(|| Device::open(device)) else { return };

            let mut delta = Vec2::ZERO;
            for report in reports.chunks_exact(hid_size) {
                if let Some(d) = device.read(report, &mut self.frame) {
                    delta += d;
                }
            }
            // Skipped while a nested message loop (a modal dialog) holds the app mutably.
            if let Ok(mut app) = self.app.try_borrow_mut() {
                app.trackpad_active = true;
                if delta != Vec2::ZERO {
                    app.send(ViewportRequest::Input(ViewportInput::Orbit { delta: delta * PX_PER_PAD }));
                }
            }
            self.idle.restart();
        }
    }

    impl Device {
        fn open(handle: HANDLE) -> Option<Device> {
            let mut size = 0u32;
            // SAFETY: a size query with a null buffer, then a fill of exactly that many bytes.
            unsafe {
                GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, null_mut(), &mut size);
                if size == 0 {
                    return None;
                }
                let mut preparsed = vec![0u64; (size as usize).div_ceil(8)];
                if GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, preparsed.as_mut_ptr().cast(), &mut size) == u32::MAX {
                    return None;
                }
                let pp = preparsed.as_ptr() as PHIDP_PREPARSED_DATA;
                let mut caps: HIDP_CAPS = zeroed();
                if HidP_GetCaps(pp, &mut caps) != HIDP_STATUS_SUCCESS {
                    return None;
                }
                let mut n = caps.NumberInputValueCaps;
                let mut values = vec![HIDP_VALUE_CAPS::default(); n as usize];
                if HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut n, pp) != HIDP_STATUS_SUCCESS {
                    return None;
                }
                values.truncate(n as usize);
                let axis = |usage: u16| {
                    values
                        .iter()
                        .filter(move |v| v.UsagePage == 0x01 && !v.IsRange && v.Anonymous.NotRange.Usage == usage && v.LogicalMax > v.LogicalMin)
                        .map(|v| (v.LinkCollection, (v.LogicalMin, v.LogicalMax)))
                };
                let fingers: Vec<_> = axis(0x30)
                    .filter_map(|(link, x)| axis(0x31).find(|(l, _)| *l == link).map(|(_, y)| (link, x, y)))
                    .collect();
                (!fingers.is_empty()).then_some(Device { preparsed, fingers })
            }
        }

        /// Reads one report into `frame`; returns the three-finger move when it completes a frame.
        fn read(&self, report: &[u8], frame: &mut Frame) -> Option<Vec2> {
            let pp = self.preparsed.as_ptr() as PHIDP_PREPARSED_DATA;
            let len = report.len() as u32;
            // HidP_GetUsages takes a mutable pointer but only reads the report.
            let ptr = report.as_ptr() as *mut u8;
            let mut count = 0u32;
            // SAFETY: `pp` is this device's preparsed data and `report` one of its input reports.
            let ok = unsafe { HidP_GetUsageValue(HidP_Input, 0x0D, 0, 0x54, &mut count, pp, ptr, len) } == HIDP_STATUS_SUCCESS;
            if ok && count > 0 {
                frame.tips.clear();
                frame.remaining = count;
            }
            if frame.remaining == 0 {
                return None;
            }
            for &(link, (x0, x1), (y0, y1)) in &self.fingers {
                if frame.remaining == 0 {
                    break;
                }
                frame.remaining -= 1;
                let mut usages = [0u16; 16];
                let mut n = usages.len() as u32;
                let (mut x, mut y) = (0u32, 0u32);
                // SAFETY: as above; `usages` holds `n` entries.
                let tip = unsafe {
                    HidP_GetUsages(HidP_Input, 0x0D, link, usages.as_mut_ptr(), &mut n, pp, ptr, len) == HIDP_STATUS_SUCCESS
                        && usages[..(n as usize).min(usages.len())].contains(&0x42)
                        && HidP_GetUsageValue(HidP_Input, 0x01, link, 0x30, &mut x, pp, ptr, len) == HIDP_STATUS_SUCCESS
                        && HidP_GetUsageValue(HidP_Input, 0x01, link, 0x31, &mut y, pp, ptr, len) == HIDP_STATUS_SUCCESS
                };
                if tip {
                    let norm = |v: u32, lo: i32, hi: i32| (v as f32 - lo as f32) / (hi - lo) as f32;
                    frame.tips.push(Vec2::new(norm(x, x0, x1), norm(y, y0, y1)));
                }
            }
            if frame.remaining > 0 {
                return None;
            }
            let (centroid, delta) = step(frame.prev, &frame.tips);
            frame.prev = centroid;
            delta
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_fingers_move_and_two_reset() {
        let three = [Vec2::new(0.1, 0.5), Vec2::new(0.2, 0.5), Vec2::new(0.3, 0.5)];
        let (c, d) = step(None, &three);
        assert!(d.is_none());
        let moved = three.map(|p| p + Vec2::new(0.05, -0.02));
        let (c, d) = step(c, &moved);
        assert!(d.unwrap().abs_diff_eq(Vec2::new(0.05, -0.02), 1e-6));
        assert_eq!(step(c, &moved[..2]), (None, None));
    }
}
