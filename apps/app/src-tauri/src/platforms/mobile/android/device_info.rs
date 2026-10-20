pub(crate) fn read_device_model() -> Option<String> {
    use std::ffi::{c_char, c_int};
    extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    // Android system property values are limited to PROP_VALUE_MAX (92 bytes).
    let mut buffer = [0u8; 92];
    let size = unsafe {
        __system_property_get(
            b"ro.product.model\0".as_ptr().cast(),
            buffer.as_mut_ptr().cast(),
        )
    };
    (size > 0 && (size as usize) < buffer.len())
        .then(|| String::from_utf8_lossy(&buffer[..size as usize]).into_owned())
}

pub(crate) fn read_memory_total_bytes() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let total = info
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?;
    let mut parts = total.split_whitespace();
    let kib = parts.next()?.parse::<u64>().ok()?;
    (parts.next()? == "kB").then_some(kib)?.checked_mul(1024)
}
