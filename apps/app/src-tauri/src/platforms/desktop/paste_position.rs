/// 所有参数使用同一屏幕坐标系，y 向下增加；不混用像素与逻辑点。
pub(super) fn calculate_position(
    cursor: (f64, f64),
    size: (f64, f64),
    work_origin: (f64, f64),
    work_size: (f64, f64),
) -> (f64, f64) {
    const MARGIN: f64 = 8.0;
    const GAP: f64 = 12.0;
    let min_x = work_origin.0 + MARGIN;
    let min_y = work_origin.1 + MARGIN;
    let max_x = (work_origin.0 + work_size.0 - size.0 - MARGIN).max(min_x);
    let max_y = (work_origin.1 + work_size.1 - size.1 - MARGIN).max(min_y);
    let right = cursor.0 + GAP;
    let below = cursor.1 + GAP;
    (
        (if right <= max_x {
            right
        } else {
            cursor.0 - size.0 - GAP
        })
        .clamp(min_x, max_x),
        (if below <= max_y {
            below
        } else {
            cursor.1 - size.1 - GAP
        })
        .clamp(min_y, max_y),
    )
}
