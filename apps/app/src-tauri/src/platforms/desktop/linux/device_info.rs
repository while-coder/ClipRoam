pub(crate) fn read_device_model() -> Option<String> {
    [
        "/sys/class/dmi/id/product_name",
        "/sys/firmware/devicetree/base/model",
    ]
    .iter()
    .find_map(|path| {
        std::fs::read_to_string(path)
            .ok()
            .filter(|s| !s.trim_matches('\0').trim().is_empty())
    })
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
