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

#[cfg(test)]
mod tests {
    use super::calculate_position;

    #[test]
    fn opens_below_and_to_the_right_of_cursor() {
        assert_eq!(
            calculate_position(
                (400.0, 200.0),
                (560.0, 640.0),
                (0.0, 24.0),
                (1920.0, 1056.0)
            ),
            (412.0, 212.0)
        );
    }

    #[test]
    fn flips_at_right_and_bottom_edges() {
        assert_eq!(
            calculate_position(
                (1900.0, 1060.0),
                (560.0, 640.0),
                (0.0, 24.0),
                (1920.0, 1056.0)
            ),
            (1328.0, 408.0)
        );
    }

    #[test]
    fn handles_monitors_left_of_or_above_primary() {
        assert_eq!(
            calculate_position(
                (-1800.0, 200.0),
                (560.0, 640.0),
                (-1920.0, 24.0),
                (1920.0, 1056.0)
            ),
            (-1788.0, 212.0)
        );
        assert_eq!(
            calculate_position(
                (400.0, -900.0),
                (560.0, 640.0),
                (0.0, -1080.0),
                (1920.0, 1080.0)
            ),
            (412.0, -888.0)
        );
    }

    #[test]
    fn clamps_to_work_area_for_small_screens() {
        assert_eq!(
            calculate_position((100.0, 100.0), (560.0, 640.0), (0.0, 24.0), (500.0, 500.0)),
            (8.0, 32.0)
        );
    }
}
