pub(crate) fn read_device_model() -> Option<String> {
    use std::ffi::{c_char, c_int, c_void};
    extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }
    let key = b"hw.machine\0";
    let mut buffer = [0u8; 256];
    let mut size = buffer.len();
    let status = unsafe {
        sysctlbyname(
            key.as_ptr().cast(),
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && size <= buffer.len())
        .then(|| String::from_utf8_lossy(&buffer[..size]).into_owned())
}

pub(crate) fn read_memory_total_bytes() -> Option<u64> {
    use std::ffi::{c_char, c_int, c_void};
    extern "C" {
        fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> c_int;
    }
    let mut bytes = 0u64;
    let mut size = std::mem::size_of_val(&bytes);
    let status = unsafe {
        sysctlbyname(
            b"hw.memsize\0".as_ptr().cast(),
            (&mut bytes as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && size == std::mem::size_of_val(&bytes)).then_some(bytes)
}
