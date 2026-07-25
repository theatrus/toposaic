const KILOMETRES_PER_LATITUDE_DEGREE: f64 = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE: f64 = 111.32;
const MINIMUM_LONGITUDE_SCALE: f64 = 20.0;
const MAX_MODEL_LATITUDE: f64 = 85.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GeoBounds {
    pub(crate) south: f64,
    pub(crate) north: f64,
    pub(crate) west: f64,
    pub(crate) east: f64,
}

impl GeoBounds {
    pub(crate) fn around(latitude: f64, longitude: f64, span_km: f64) -> Self {
        let half_latitude = span_km / 2.0 / KILOMETRES_PER_LATITUDE_DEGREE;
        let half_longitude = longitude_degrees(span_km / 2.0, latitude);
        Self {
            south: (latitude - half_latitude).max(-MAX_MODEL_LATITUDE),
            north: (latitude + half_latitude).min(MAX_MODEL_LATITUDE),
            west: longitude - half_longitude,
            east: longitude + half_longitude,
        }
    }

    pub(crate) fn split_at_antimeridian(self) -> Vec<Self> {
        if self.west < -180.0 {
            vec![
                Self {
                    west: self.west + 360.0,
                    east: 180.0,
                    ..self
                },
                Self {
                    west: -180.0,
                    ..self
                },
            ]
        } else if self.east > 180.0 {
            vec![
                Self {
                    east: 180.0,
                    ..self
                },
                Self {
                    west: -180.0,
                    east: self.east - 360.0,
                    ..self
                },
            ]
        } else {
            vec![self]
        }
    }
}

pub(crate) fn offset_coordinates(
    latitude: f64,
    longitude: f64,
    north_km: f64,
    east_km: f64,
) -> (f64, f64) {
    let longitude = normalize_longitude(longitude + longitude_degrees(east_km, latitude));
    let latitude = (latitude + north_km / KILOMETRES_PER_LATITUDE_DEGREE)
        .clamp(-MAX_MODEL_LATITUDE, MAX_MODEL_LATITUDE);
    (latitude, longitude)
}

pub(crate) fn normalize_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

fn longitude_degrees(distance_km: f64, latitude: f64) -> f64 {
    let scale = (KILOMETRES_PER_LONGITUDE_DEGREE * latitude.to_radians().cos().abs())
        .max(MINIMUM_LONGITUDE_SCALE);
    distance_km / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_longitudes_at_the_date_line() {
        assert!((normalize_longitude(181.0) + 179.0).abs() < f64::EPSILON);
        assert!((normalize_longitude(-181.0) - 179.0).abs() < f64::EPSILON);
    }

    #[test]
    fn square_bounds_and_offsets_use_the_same_distance_rules() {
        let bounds = GeoBounds::around(46.8523, -121.7603, 18.0);
        let (south, west) = offset_coordinates(46.8523, -121.7603, -9.0, -9.0);
        let (north, east) = offset_coordinates(46.8523, -121.7603, 9.0, 9.0);

        assert!((bounds.south - south).abs() < 0.000_001);
        assert!((bounds.north - north).abs() < 0.000_001);
        assert!((bounds.west - west).abs() < 0.000_1);
        assert!((bounds.east - east).abs() < 0.000_1);
    }

    #[test]
    fn splits_bounds_that_cross_the_antimeridian() {
        let parts = GeoBounds::around(0.0, 179.95, 20.0).split_at_antimeridian();

        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].east, 180.0);
        assert_eq!(parts[1].west, -180.0);
        assert!(parts[1].east < -179.0);
    }
}
