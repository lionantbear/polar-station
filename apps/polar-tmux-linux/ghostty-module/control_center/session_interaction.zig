//! Session-list viewport preservation.
//!
//! A Session row has one pointer meaning: immediately toggle its assignment.
//! There is no click-delay or double-click arbitration in the native adapter.

pub fn restoredScrollOffset(
    previous: f64,
    upper: f64,
    page_size: f64,
) f64 {
    const maximum = @max(@as(f64, 0), upper - page_size);
    return @max(@as(f64, 0), @min(previous, maximum));
}
