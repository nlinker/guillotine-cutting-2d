use super::{ProgressMessage, ProgressSink};

// == Windows API FFI ========================================================

type Handle = *mut std::ffi::c_void;
type Bool = i32;
type Dword = u32;
type Lpcwstr = *const u16;

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const PIPE_ACCESS_OUTBOUND: Dword = 0x00000002;
const PIPE_TYPE_MESSAGE: Dword = 0x00000004;
const PIPE_READMODE_MESSAGE: Dword = 0x00000002;
const PIPE_WAIT: Dword = 0x00000000;
const NMPWAIT_USE_DEFAULT_WAIT: Dword = 0x00000000;
const FILE_FLAG_FIRST_PIPE_INSTANCE: Dword = 0x00080000;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateNamedPipeW(
        lp_name: Lpcwstr,
        dw_open_mode: Dword,
        dw_pipe_mode: Dword,
        n_max_instances: Dword,
        n_out_buffer_size: Dword,
        n_in_buffer_size: Dword,
        n_default_time_out: Dword,
        lp_security_attributes: *mut std::ffi::c_void,
    ) -> Handle;

    fn ConnectNamedPipe(h_named_pipe: Handle, lp_overlapped: *mut std::ffi::c_void) -> Bool;

    fn WriteFile(
        h_file: Handle,
        lp_buffer: *const std::ffi::c_void,
        n_number_of_bytes_to_write: Dword,
        lp_number_of_bytes_written: *mut Dword,
        lp_overlapped: *mut std::ffi::c_void,
    ) -> Bool;

    fn FlushFileBuffers(h_file: Handle) -> Bool;
    fn DisconnectNamedPipe(h_named_pipe: Handle) -> Bool;
    fn CloseHandle(h_object: Handle) -> Bool;
    fn GetLastError() -> Dword;
}

fn to_wstring(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect()
}

// == Sink ==================================================================

pub struct WindowsPipeSink {
    handle: Handle,
}

// SAFETY: Handle is a raw pointer to a kernel object. We never share it across
// threads — the sink is moved into the event loop thread and used from there only.
unsafe impl Send for WindowsPipeSink {}

impl WindowsPipeSink {
    /// Create the named pipe and block until a client connects.
    pub fn create_and_wait(name: &str) -> std::io::Result<Self> {
        let name_w = to_wstring(name);
        let handle = unsafe {
            CreateNamedPipeW(
                name_w.as_ptr(),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                1,
                65536,
                0,
                NMPWAIT_USE_DEFAULT_WAIT,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) };
        if connected == 0 {
            // ERROR_PIPE_CONNECTED (535) means client already connected — that is success.
            let err = unsafe { GetLastError() };
            if err != 535 {
                unsafe { CloseHandle(handle) };
                return Err(std::io::Error::from_raw_os_error(err as i32));
            }
        }
        Ok(Self { handle })
    }

    fn write_message(&self, msg: &str) -> bool {
        let bytes = msg.as_bytes();
        let mut written: Dword = 0;
        let ok = unsafe {
            WriteFile(
                self.handle,
                bytes.as_ptr() as *const std::ffi::c_void,
                bytes.len() as Dword,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        unsafe { FlushFileBuffers(self.handle) };
        ok != 0
    }
}

impl ProgressSink for WindowsPipeSink {
    fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
        if self.write_message(&msg.to_line()) {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

impl Drop for WindowsPipeSink {
    fn drop(&mut self) {
        unsafe {
            DisconnectNamedPipe(self.handle);
            CloseHandle(self.handle);
        }
    }
}
