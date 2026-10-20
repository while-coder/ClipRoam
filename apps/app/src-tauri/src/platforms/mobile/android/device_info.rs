pub(crate) fn read_device_model() -> Option<String> {
    read_system_property(b"ro.product.model\0")
}

pub(crate) fn read_device_market_name() -> Option<String> {
    // These are vendor properties, not an Android SDK contract. Keep the raw
    // model separately and omit the marketing name when the ROM exposes none.
    [
        b"ro.product.marketname\0".as_slice(),
        b"ro.vendor.oplus.market.name\0",
        b"ro.vendor.oplus.market.enname\0",
        b"ro.oppo.market.name\0",
        b"ro.vivo.market.name\0",
        b"ro.config.marketing_name\0",
        b"ro.product.vendor.marketname\0",
    ]
    .into_iter()
    .find_map(read_system_property)
}

fn read_system_property(name: &[u8]) -> Option<String> {
    use std::ffi::{c_char, c_int};
    extern "C" {
        fn __system_property_get(name: *const c_char, value: *mut c_char) -> c_int;
    }
    // Android system property values are limited to PROP_VALUE_MAX (92 bytes).
    let mut buffer = [0u8; 92];
    let size = unsafe {
        __system_property_get(
            name.as_ptr().cast(),
            buffer.as_mut_ptr().cast(),
        )
    };
    if size <= 0 || size as usize >= buffer.len() {
        return None;
    }
    let value = String::from_utf8_lossy(&buffer[..size as usize]).trim().to_string();
    (!value.is_empty() && !value.eq_ignore_ascii_case("unknown")).then_some(value)
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
