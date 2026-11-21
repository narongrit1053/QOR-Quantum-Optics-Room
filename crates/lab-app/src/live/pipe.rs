//! Local named-pipe transport for live control: one JSON line in, one JSON line out.
//! Each pipe instance is served by blocking I/O on its own background thread, never the UI thread.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FindClose, FindFirstFileW, FindNextFileW,
    FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WIN32_FIND_DATAW, WriteFile,
};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;

use super::protocol::{ErrorCode, LiveError, MAX_LINE, err_line};

const INSTANCES: u32 = 4;
const BUF: u32 = 64 * 1024;

type Handler = dyn Fn(String) -> Receiver<String> + Send + Sync;

/// A kernel handle closed on drop.
struct Owned(HANDLE);
// SAFETY: a kernel handle is a process-wide value, usable from any thread.
unsafe impl Send for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we own this handle and close it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Memory from LocalAlloc (security descriptors, SID strings) freed on drop.
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the pointer came from an API that documents LocalFree as its release.
        unsafe { LocalFree(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn user_name() -> String {
    let mut buf = [0u16; 257]; // UNLEN + 1
    let mut len = buf.len() as u32;
    // SAFETY: buf is writable for len u16s; len receives the count including the NUL.
    let ok = unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) } != 0;
    let s = if ok { String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]) } else { String::new() };
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' })
        .collect()
}

pub fn pipe_name(pid: u32) -> String {
    format!(r"\\.\pipe\labsetup-{}-{pid}", user_name())
}

/// The process token's TOKEN_USER, in a u64 buffer so the struct is aligned.
fn token_user() -> io::Result<Vec<u64>> {
    let mut tok = null_mut();
    // SAFETY: GetCurrentProcess is a pseudo-handle; tok receives a handle we then own.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let tok = Owned(tok);
    let mut len = 0u32;
    // SAFETY: size query with no buffer; it fails by design and sets len.
    unsafe { GetTokenInformation(tok.0, TokenUser, null_mut(), 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: buf holds at least len bytes, 8-aligned for TOKEN_USER.
    if unsafe { GetTokenInformation(tok.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf)
}

fn sid_of(token_user: &[u64]) -> PSID {
    // SAFETY: the buffer was filled by GetTokenInformation(TokenUser); the SID lives inside it.
    unsafe { (*token_user.as_ptr().cast::<TOKEN_USER>()).User.Sid }
}

/// `D:P(A;;GA;;;<user sid>)`: a protected DACL granting only the current user.
fn owner_only_sd() -> io::Result<Local> {
    let user = token_user()?;
    let mut s = null_mut();
    // SAFETY: the SID is valid; s receives a LocalAlloc'd NUL-terminated string.
    if unsafe { ConvertSidToStringSidW(sid_of(&user), &mut s) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let s = Local(s.cast());
    let p = s.0.cast::<u16>();
    // SAFETY: p is NUL-terminated, so the scan stops inside the allocation.
    let sid = unsafe {
        let n = (0..).take_while(|&i| *p.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    };
    let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
    let mut sd = null_mut();
    // SAFETY: sddl is NUL-terminated; sd receives a LocalAlloc'd descriptor.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Local(sd))
}

fn write_all(h: HANDLE, mut data: &[u8]) -> bool {
    while !data.is_empty() {
        let mut n = 0u32;
        let len = data.len().min(u32::MAX as usize) as u32;
        // SAFETY: data is readable for len bytes; synchronous handle, no OVERLAPPED.
        if unsafe { WriteFile(h, data.as_ptr(), len, &mut n, null_mut()) } == 0 || n == 0 {
            return false;
        }
        data = &data[n as usize..];
    }
    true
}

/// Appends what one ReadFile returns to `buf`; false on EOF or error.
fn read_some(h: HANDLE, buf: &mut Vec<u8>, chunk: &mut [u8]) -> bool {
    let mut n = 0u32;
    // SAFETY: chunk is writable for its full length; synchronous handle, no OVERLAPPED.
    let ok = unsafe { ReadFile(h, chunk.as_mut_ptr(), chunk.len() as u32, &mut n, null_mut()) } != 0;
    buf.extend_from_slice(&chunk[..n as usize]);
    ok && n > 0
}

struct Shared {
    stop: AtomicBool,
    last_seen: Mutex<Option<Instant>>,
}

pub struct Server {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
    pub fn start(
        name: &str,
        handle: impl Fn(String) -> Receiver<String> + Send + Sync + 'static,
    ) -> io::Result<Server> {
        let sd = owner_only_sd()?;
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let wname = wide(name);
        let handler: Arc<Handler> = Arc::new(handle);
        let mut server = Server {
            shared: Arc::new(Shared { stop: AtomicBool::new(false), last_seen: Mutex::new(None) }),
            threads: Vec::new(),
        };
        for i in 0..INSTANCES {
            // The first instance must be new, so we never join a pipe someone else squatted.
            let first = if i == 0 { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
            // SAFETY: wname is NUL-terminated; sa and the descriptor it points to outlive the call.
            let h = unsafe {
                CreateNamedPipeW(
                    wname.as_ptr(),
                    PIPE_ACCESS_DUPLEX | first,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    INSTANCES,
                    BUF,
                    BUF,
                    0,
                    &sa,
                )
            };
            if h == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error()); // drop stops the instances already running
            }
            let pipe = Owned(h);
            let (shared, handler) = (server.shared.clone(), handler.clone());
            let t = std::thread::Builder::new()
                .name("live-pipe".into())
                .spawn(move || serve(&pipe, &shared, &*handler))?;
            server.threads.push(t);
        }
        Ok(server)
    }

    pub fn last_seen(&self) -> Option<Instant> {
        self.shared.last_seen.lock().map(|g| *g).unwrap_or(None)
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            // Blocking pipe calls (ConnectNamedPipe, ReadFile, ...) never see the flag, so keep
            // cancelling them with CancelSynchronousIo until the thread notices it and exits.
            while !t.is_finished() {
                // SAFETY: the JoinHandle keeps the thread handle open until join.
                unsafe { CancelSynchronousIo(t.as_raw_handle()) };
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = t.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve(pipe: &Owned, shared: &Shared, handler: &Handler) {
    while !shared.stop.load(Ordering::SeqCst) {
        // SAFETY: pipe is a server instance opened without FILE_FLAG_OVERLAPPED; GetLastError
        // only reads this thread's last error.
        let connected = unsafe { ConnectNamedPipe(pipe.0, null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
        if connected {
            session(pipe.0, shared, handler);
            // SAFETY: lets the client read our last bytes before the disconnect discards them.
            unsafe { FlushFileBuffers(pipe.0) };
        } else {
            std::thread::sleep(Duration::from_millis(10)); // cancelled, or a client already gone
        }
        // SAFETY: resets the instance so the next client can connect; harmless if not connected.
        unsafe { DisconnectNamedPipe(pipe.0) };
    }
}

/// Serves lines until the client leaves, a write fails, or a line is too long.
fn session(h: HANDLE, shared: &Shared, handler: &Handler) {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; BUF as usize];
    loop {
        while let Some(i) = buf.iter().position(|&b| b == b'\n') {
            if i > MAX_LINE {
                break;
            }
            let line: Vec<u8> = buf.drain(..=i).collect();
            if let Ok(mut g) = shared.last_seen.lock() {
                *g = Some(Instant::now());
            }
            let rx = handler(String::from_utf8_lossy(&line[..i]).into_owned());
            let Some(reply) = wait_reply(&rx, shared) else { return };
            if !write_all(h, reply.as_bytes()) {
                return; // client gone before the reply
            }
        }
        if buf.len() > MAX_LINE {
            let e = LiveError::new(ErrorCode::BadRequest, "request too long");
            write_all(h, err_line("", &e).as_bytes());
            return;
        }
        if !read_some(h, &mut buf, &mut chunk) {
            return;
        }
    }
}

fn wait_reply(rx: &Receiver<String>, shared: &Shared) -> Option<String> {
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => return Some(r),
            Err(RecvTimeoutError::Timeout) if !shared.stop.load(Ordering::SeqCst) => {}
            Err(_) => return None,
        }
    }
}

/// Live-control pipes of the current user, as (pid, full pipe name).
pub fn find_pipes() -> Vec<(u32, String)> {
    let prefix = format!("labsetup-{}-", user_name());
    let pattern = wide(r"\\.\pipe\*");
    // SAFETY: WIN32_FIND_DATAW is plain data; all-zero is a valid value.
    let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    // SAFETY: pattern is NUL-terminated; fd is writable.
    let h = unsafe { FindFirstFileW(pattern.as_ptr(), &mut fd) };
    if h == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    loop {
        let len = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
        let n = String::from_utf16_lossy(&fd.cFileName[..len]);
        if let Some(pid) = n.strip_prefix(&prefix).and_then(|p| p.parse().ok()) {
            out.push((pid, format!(r"\\.\pipe\{n}")));
        }
        // SAFETY: h is the live find handle from FindFirstFileW.
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            break;
        }
    }
    // SAFETY: closes the find handle exactly once (find handles are not closed with CloseHandle).
    unsafe { FindClose(h) };
    out
}

fn connect(name: &str, timeout: Duration) -> io::Result<Owned> {
    let w = wide(name);
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: w is NUL-terminated. Identification-level SQOS stops a server impersonating us.
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            return Ok(Owned(h));
        }
        let e = io::Error::last_os_error();
        let left = deadline.saturating_duration_since(Instant::now());
        if e.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) || left.is_zero() {
            return Err(e);
        }
        let ms = left.as_millis().clamp(1, u32::MAX as u128) as u32; // 0 would mean "default wait"
        // SAFETY: w is NUL-terminated.
        unsafe { WaitNamedPipeW(w.as_ptr(), ms) };
    }
}

/// Sends one line and returns the one-line reply without its newline.
pub fn call(name: &str, line: &str, timeout: Duration) -> io::Result<String> {
    let pipe = connect(name, timeout)?;
    let mut msg = line.to_owned();
    if !msg.ends_with('\n') {
        msg.push('\n');
    }
    if !write_all(pipe.0, msg.as_bytes()) {
        return Err(io::Error::last_os_error());
    }
    let (mut out, mut chunk) = (Vec::new(), vec![0u8; BUF as usize]);
    loop {
        let start = out.len();
        let more = read_some(pipe.0, &mut out, &mut chunk);
        if let Some(i) = out[start..].iter().position(|&b| b == b'\n') {
            out.truncate(start + i);
            return String::from_utf8(out).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        }
        if !more {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed before a reply"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::protocol::MAX_LINE;
    use std::sync::mpsc;

    const T: Duration = Duration::from_secs(5);

    fn name(suffix: &str) -> String {
        format!("{}-{suffix}", pipe_name(std::process::id()))
    }

    fn echo(line: String) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        tx.send(format!("{line}\n")).unwrap();
        rx
    }

    #[test]
    fn echo_round_trip() {
        let n = name("echo");
        let s = Server::start(&n, echo).unwrap();
        assert!(s.last_seen().is_none());
        assert_eq!(call(&n, r#"{"v":1,"cmd":"list"}"#, T).unwrap(), r#"{"v":1,"cmd":"list"}"#);
        assert!(s.last_seen().is_some());
        s.stop();
        assert!(call(&n, "x", Duration::from_millis(100)).is_err(), "pipe gone after stop");
    }

    #[test]
    fn two_clients_at_once() {
        let n = name("two");
        let _s = Server::start(&n, echo).unwrap();
        let _held = connect(&n, T).unwrap();
        assert_eq!(call(&n, "second", T).unwrap(), "second");
    }

    #[test]
    fn oversized_line_is_bad_request() {
        let n = name("big");
        let _s = Server::start(&n, echo).unwrap();
        let reply = call(&n, &"a".repeat(MAX_LINE + 100), T).unwrap();
        assert!(reply.contains(r#""code":"bad_request""#), "{reply}");
        assert!(reply.contains("request too long"), "{reply}");
        assert_eq!(call(&n, "after", T).unwrap(), "after");
    }

    #[test]
    fn client_gone_before_reply_keeps_serving() {
        let n = name("gone");
        let _s = Server::start(&n, |line: String| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                let _ = tx.send(format!("{line}\n"));
            });
            rx
        })
        .unwrap();
        // Occupy every instance with a client that leaves before its reply.
        for _ in 0..INSTANCES {
            let c = connect(&n, T).unwrap();
            assert!(write_all(c.0, b"bye\n"));
        }
        std::thread::sleep(Duration::from_millis(500));
        // All four instances must be waiting again: three held plus one call.
        let _held: Vec<_> = (0..INSTANCES - 1).map(|_| connect(&n, T).unwrap()).collect();
        assert_eq!(call(&n, "still here", T).unwrap(), "still here");
    }

    #[test]
    fn pipe_dacl_is_owner_only() {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION, EqualSid, GetAce,
        };
        let n = name("dacl");
        let _s = Server::start(&n, echo).unwrap();
        // A client handle names the same pipe object, so it carries the server's descriptor.
        let c = connect(&n, T).unwrap();
        let user = token_user().unwrap();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = std::ptr::null_mut();
        // SAFETY: c is an open pipe handle with READ_CONTROL; out-pointers are valid.
        let rc = unsafe {
            GetSecurityInfo(
                c.0,
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        assert_eq!(rc, 0, "GetSecurityInfo");
        // SAFETY: dacl points into sd, which stays alive until LocalFree below.
        unsafe {
            assert_eq!((*dacl).AceCount, 1);
            let mut ace = std::ptr::null_mut();
            assert_ne!(GetAce(dacl, 0, &mut ace), 0);
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!((*ace).Header.AceType, 0, "ACCESS_ALLOWED_ACE_TYPE");
            let sid = std::ptr::addr_of!((*ace).SidStart).cast_mut().cast();
            assert_ne!(EqualSid(sid, sid_of(&user)), 0, "ACE is for the current user");
            LocalFree(sd);
        }
    }

    #[test]
    fn find_pipes_lists_own_pid() {
        let n = pipe_name(std::process::id());
        let _s = Server::start(&n, echo).unwrap();
        assert!(find_pipes().contains(&(std::process::id(), n)));
    }
}
