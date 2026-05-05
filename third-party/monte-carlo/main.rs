/// pi_solver — Monte Carlo estimation of PI with progress delivered via named pipe.
///
/// Pipe protocol (each message is a line ending with \n):
///   PROGRESS|<second>|<iterations>|<pi_estimate>|<error>
///   DONE|<iterations>|<final_estimate>
///   ERROR|<message>
///
/// Pipe: \\.\pipe\pi_solver_progress
///
/// Usage:
///   pi_solver.exe [seconds]
///   pi_solver.exe          — runs for 60 seconds (default)
///   pi_solver.exe 30       — runs for 30 seconds

use std::ffi::OsStr;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::time::{Duration, Instant};

// === Windows API FFI ========================================================

type HANDLE  = *mut std::ffi::c_void;
type BOOL    = i32;
type DWORD   = u32;
type LPCWSTR = *const u16;

const INVALID_HANDLE_VALUE: HANDLE = -1isize as HANDLE;
const GENERIC_WRITE:        DWORD  = 0x40000000;
const OPEN_EXISTING:        DWORD  = 3;
const FILE_ATTRIBUTE_NORMAL: DWORD = 0x80;
const PIPE_ACCESS_OUTBOUND:  DWORD = 0x00000002;
const PIPE_TYPE_MESSAGE:     DWORD = 0x00000004;
const PIPE_READMODE_MESSAGE: DWORD = 0x00000002;
const PIPE_WAIT:             DWORD = 0x00000000;
const NMPWAIT_USE_DEFAULT_WAIT: DWORD = 0x00000000;
const FILE_FLAG_FIRST_PIPE_INSTANCE: DWORD = 0x00080000;

#[link(name = "kernel32")]
extern "system" {
    fn CreateNamedPipeW(
        lpName:               LPCWSTR,
        dwOpenMode:           DWORD,
        dwPipeMode:           DWORD,
        nMaxInstances:        DWORD,
        nOutBufferSize:       DWORD,
        nInBufferSize:        DWORD,
        nDefaultTimeOut:      DWORD,
        lpSecurityAttributes: *mut std::ffi::c_void,
    ) -> HANDLE;

    fn ConnectNamedPipe(hNamedPipe: HANDLE, lpOverlapped: *mut std::ffi::c_void) -> BOOL;
    fn WriteFile(
        hFile:                  HANDLE,
        lpBuffer:               *const std::ffi::c_void,
        nNumberOfBytesToWrite:  DWORD,
        lpNumberOfBytesWritten: *mut DWORD,
        lpOverlapped:           *mut std::ffi::c_void,
    ) -> BOOL;
    fn CloseHandle(hObject: HANDLE) -> BOOL;
    fn FlushFileBuffers(hFile: HANDLE) -> BOOL;
    fn DisconnectNamedPipe(hNamedPipe: HANDLE) -> BOOL;
}

// === Helpers ================================================================

/// Converts &str to a null-terminated UTF-16 string for WinAPI.
fn to_wstring(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect()
}

/// Writes a string to the named pipe.
fn write_message(pipe: HANDLE, msg: &str) -> bool {
    let bytes = msg.as_bytes();
    let mut written: DWORD = 0;
    let ok = unsafe {
        WriteFile(
            pipe,
            bytes.as_ptr() as *const std::ffi::c_void,
            bytes.len() as DWORD,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    unsafe { FlushFileBuffers(pipe) };
    ok != 0
}

// === LCG pseudo-random number generator =====================================
/// Simple and fast; no external crate needed.
/// Returns f64 in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }

    fn next_f64(&mut self) -> f64 {
        self.0 = self.0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Use the top 53 bits for f64 in [0, 1)
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

// === Monte Carlo =============================================================

const PIPE_NAME: &str = r"\\.\pipe\pi_solver_progress";
const BATCH:     u64  = 500_000; // iterations between progress updates

fn main() {
    // Parse arguments
    let total_secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    eprintln!("[pi_solver] Starting. Running for {total_secs}s, pipe: {PIPE_NAME}");

    // == Create the named pipe ==============================================
    let pipe_name_w = to_wstring(PIPE_NAME);

    let pipe = unsafe {
        CreateNamedPipeW(
            pipe_name_w.as_ptr(),
            PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
            1,          // max 1 instance
            65536,      // output buffer
            0,          // input buffer (not needed)
            NMPWAIT_USE_DEFAULT_WAIT,
            std::ptr::null_mut(),
        )
    };

    if pipe == INVALID_HANDLE_VALUE {
        eprintln!("[pi_solver] Error: failed to create pipe.");
        std::process::exit(1);
    }

    eprintln!("[pi_solver] Pipe created. Waiting for client...");

    // == Wait for client (VBA) to connect ==================================
    let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) };
    if connected == 0 {
        // ERROR_PIPE_CONNECTED (535) also means success — client already connected
        let err = unsafe { windows_get_last_error() };
        if err != 535 {
            eprintln!("[pi_solver] ConnectNamedPipe error: {err}");
            unsafe { CloseHandle(pipe) };
            std::process::exit(1);
        }
    }

    eprintln!("[pi_solver] Client connected. Starting calculation...");

    // == Monte Carlo ========================================================
    let mut rng    = Lcg::new(0xDEAD_BEEF_1337_1337);
    let mut inside: u64 = 0;
    let mut total:  u64 = 0;
    let start = Instant::now();
    let deadline = Duration::from_secs(total_secs);

    loop {
        // Batch of iterations
        for _ in 0..BATCH {
            let x = rng.next_f64();
            let y = rng.next_f64();
            if x * x + y * y <= 1.0 {
                inside += 1;
            }
        }
        total += BATCH;

        let elapsed = start.elapsed();
        let secs    = elapsed.as_secs();
        let pi_est  = 4.0 * inside as f64 / total as f64;
        let err     = (pi_est - std::f64::consts::PI).abs();

        // Send progress
        let msg = format!("PROGRESS|{secs}|{total}|{pi_est:.10}|{err:.2e}\n");
        if !write_message(pipe, &msg) {
            eprintln!("[pi_solver] Client disconnected, stopping.");
            break;
        }

        eprintln!("[pi_solver] t={secs}s iter={total} pi≈{pi_est:.8} err={err:.2e}");

        // Simulate heavy computation — sleep 1 second
        // (in a real algorithm this would be something computationally expensive)
        std::thread::sleep(Duration::from_secs(1));

        if elapsed >= deadline {
            // Final message
            let done = format!("DONE|{total}|{pi_est:.10}\n");
            write_message(pipe, &done);
            eprintln!("[pi_solver] Done! Final PI estimate = {pi_est:.10}");
            break;
        }
    }

    // == Close the pipe =====================================================
    unsafe {
        DisconnectNamedPipe(pipe);
        CloseHandle(pipe);
    }
}

// GetLastError without importing the full windows crate
#[link(name = "kernel32")]
extern "system" {
    fn GetLastError() -> DWORD;
}

unsafe fn windows_get_last_error() -> DWORD {
    GetLastError()
}
