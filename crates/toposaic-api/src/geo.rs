pub(crate) use toposaic_core::{GeoBounds, GeoTransform, normalize_longitude};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_longitudes_at_the_date_line() {
        assert!((normalize_longitude(181.0) + 179.0).abs() < f64::EPSILON);
        assert!((normalize_longitude(-181.0) - 179.0).abs() < f64::EPSILON);
    }

    #[test]
    fn splits_bounds_that_cross_the_antimeridian() {
        let parts = GeoBounds::around(0.0, 179.95, 20.0).split_at_antimeridian();

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].east, 180.0);
        assert_eq!(parts[1].west, -180.0);
        assert!(parts[1].east < -179.0);
    }

    #[test]
    fn rotated_tile_offsets_follow_the_model_axes() {
        let transform = GeoTransform::new(0.0, 0.0, 18.0, 90.0);
        let (latitude, longitude) = transform.coordinate_at_local_offset(0.0, 18.0);
        assert!(longitude > 0.0);
        assert!(latitude.abs() < 0.000_001);
    }
}
