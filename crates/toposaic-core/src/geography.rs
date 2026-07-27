const KILOMETRES_PER_LATITUDE_DEGREE: f64 = 110.574;
const KILOMETRES_PER_LONGITUDE_DEGREE: f64 = 111.32;
const MINIMUM_LONGITUDE_SCALE: f64 = 20.0;
const MAX_MODEL_LATITUDE: f64 = 85.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoBounds {
    pub south: f64,
    pub north: f64,
    pub west: f64,
    pub east: f64,
}

impl GeoBounds {
    pub fn around(latitude: f64, longitude: f64, span_km: f64) -> Self {
        let half_latitude = span_km / 2.0 / KILOMETRES_PER_LATITUDE_DEGREE;
        let half_longitude = longitude_degrees(span_km / 2.0, latitude);
        Self {
            south: (latitude - half_latitude).max(-MAX_MODEL_LATITUDE),
            north: (latitude + half_latitude).min(MAX_MODEL_LATITUDE),
            west: longitude - half_longitude,
            east: longitude + half_longitude,
        }
    }

    pub fn split_at_antimeridian(self) -> Vec<Self> {
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

/// Converts between geographic coordinates and the model's fixed UV frame.
/// Rotation happens here, before any mesh or print geometry sees the data.
#[derive(Debug, Clone, Copy)]
pub struct GeoTransform {
    center_latitude: f64,
    center_longitude: f64,
    span_km: f64,
    rotation_degrees: f64,
    rotation_sine: f64,
    rotation_cosine: f64,
    longitude_scale: f64,
}

impl GeoTransform {
    pub fn new(
        center_latitude: f64,
        center_longitude: f64,
        span_km: f64,
        rotation_degrees: f64,
    ) -> Self {
        Self::with_reference_latitude(
            center_latitude,
            center_longitude,
            span_km,
            rotation_degrees,
            center_latitude,
        )
    }

    /// Builds a transform whose east-west scale comes from a shared map
    /// projection. Adjacent tiles use the same reference latitude so their
    /// sampled edges meet exactly after rotation.
    pub fn with_reference_latitude(
        center_latitude: f64,
        center_longitude: f64,
        span_km: f64,
        rotation_degrees: f64,
        reference_latitude: f64,
    ) -> Self {
        let rotation_degrees = canonical_rotation(rotation_degrees);
        let (rotation_sine, rotation_cosine) = rotation_degrees.to_radians().sin_cos();
        Self {
            center_latitude,
            center_longitude,
            span_km,
            rotation_degrees,
            rotation_sine,
            rotation_cosine,
            longitude_scale: longitude_scale(reference_latitude),
        }
    }

    pub fn is_north_up(self) -> bool {
        self.rotation_degrees.abs() < f64::EPSILON
    }

    /// Geographic point at one model-space UV coordinate. U grows toward
    /// the model's right edge and V toward its top edge.
    pub fn coordinate_at_uv(self, u: f64, v: f64) -> (f64, f64) {
        if self.is_north_up() {
            let bounds =
                GeoBounds::around(self.center_latitude, self.center_longitude, self.span_km);
            return (
                bounds.south + (bounds.north - bounds.south) * v,
                normalize_longitude(bounds.west + (bounds.east - bounds.west) * u),
            );
        }

        let local_east_km = (u - 0.5) * self.span_km;
        let local_north_km = (v - 0.5) * self.span_km;
        self.coordinate_at_local_offset(local_east_km, local_north_km)
    }

    /// Geographic point at a model-space offset from the center. A positive
    /// north offset follows the rotated top edge, not true north.
    pub fn coordinate_at_local_offset(self, local_east_km: f64, local_north_km: f64) -> (f64, f64) {
        let east_km = local_east_km * self.rotation_cosine + local_north_km * self.rotation_sine;
        let north_km = -local_east_km * self.rotation_sine + local_north_km * self.rotation_cosine;
        let latitude = (self.center_latitude + north_km / KILOMETRES_PER_LATITUDE_DEGREE)
            .clamp(-MAX_MODEL_LATITUDE, MAX_MODEL_LATITUDE);
        let longitude = normalize_longitude(self.center_longitude + east_km / self.longitude_scale);
        (latitude, longitude)
    }

    /// Maps a geographic point into the fixed model UV frame. Values outside
    /// 0..=1 remain outside so downstream rasterizers can clip them.
    pub fn normalized_point(self, latitude: f64, longitude: f64) -> [f32; 2] {
        if self.is_north_up() {
            let bounds =
                GeoBounds::around(self.center_latitude, self.center_longitude, self.span_km);
            let longitude = unwrap_longitude(longitude, self.center_longitude);
            return [
                ((longitude - bounds.west) / (bounds.east - bounds.west)) as f32,
                ((latitude - bounds.south) / (bounds.north - bounds.south)) as f32,
            ];
        }

        let world_east_km = (unwrap_longitude(longitude, self.center_longitude)
            - self.center_longitude)
            * self.longitude_scale;
        let world_north_km = (latitude - self.center_latitude) * KILOMETRES_PER_LATITUDE_DEGREE;
        let local_east_km =
            world_east_km * self.rotation_cosine - world_north_km * self.rotation_sine;
        let local_north_km =
            world_east_km * self.rotation_sine + world_north_km * self.rotation_cosine;
        [
            (local_east_km / self.span_km + 0.5) as f32,
            (local_north_km / self.span_km + 0.5) as f32,
        ]
    }

    /// Axis-aligned geographic envelope used only to fetch source data. The
    /// fetched vectors are transformed and clipped in model space afterward.
    pub fn bounds(self) -> GeoBounds {
        if self.is_north_up() {
            return GeoBounds::around(self.center_latitude, self.center_longitude, self.span_km);
        }
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        corners.into_iter().fold(
            GeoBounds {
                south: f64::INFINITY,
                north: f64::NEG_INFINITY,
                west: f64::INFINITY,
                east: f64::NEG_INFINITY,
            },
            |bounds, (u, v)| {
                let (latitude, longitude) = self.coordinate_at_uv(u, v);
                let longitude = unwrap_longitude(longitude, self.center_longitude);
                GeoBounds {
                    south: bounds.south.min(latitude),
                    north: bounds.north.max(latitude),
                    west: bounds.west.min(longitude),
                    east: bounds.east.max(longitude),
                }
            },
        )
    }
}

pub fn normalize_longitude(longitude: f64) -> f64 {
    (longitude + 180.0).rem_euclid(360.0) - 180.0
}

fn unwrap_longitude(longitude: f64, center: f64) -> f64 {
    center + normalize_longitude(longitude - center)
}

fn longitude_scale(latitude: f64) -> f64 {
    (KILOMETRES_PER_LONGITUDE_DEGREE * latitude.to_radians().cos().abs())
        .max(MINIMUM_LONGITUDE_SCALE)
}

fn longitude_degrees(distance_km: f64, latitude: f64) -> f64 {
    distance_km / longitude_scale(latitude)
}

fn canonical_rotation(rotation_degrees: f64) -> f64 {
    let rotation = (rotation_degrees + 180.0).rem_euclid(360.0) - 180.0;
    if rotation.abs() < f64::EPSILON {
        0.0
    } else {
        rotation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn north_up_matches_the_old_axis_aligned_bounds() {
        let transform = GeoTransform::new(46.8523, -121.7603, 18.0, 0.0);
        let bounds = transform.bounds();
        let south_west = transform.coordinate_at_uv(0.0, 0.0);
        let north_east = transform.coordinate_at_uv(1.0, 1.0);
        assert_eq!(south_west.0, bounds.south);
        assert!((south_west.1 - bounds.west).abs() < 0.000_000_1);
        assert_eq!(north_east.0, bounds.north);
        assert!((north_east.1 - bounds.east).abs() < 0.000_000_1);
    }

    #[test]
    fn arbitrary_rotation_round_trips_model_coordinates() {
        let transform = GeoTransform::new(46.8523, -121.7603, 18.0, 37.25);
        for (u, v) in [(0.0, 0.0), (0.2, 0.8), (0.5, 0.5), (1.0, 1.0)] {
            let (latitude, longitude) = transform.coordinate_at_uv(u, v);
            let point = transform.normalized_point(latitude, longitude);
            assert!((f64::from(point[0]) - u).abs() < 0.000_01);
            assert!((f64::from(point[1]) - v).abs() < 0.000_01);
        }
    }

    #[test]
    fn quarter_turn_points_the_model_top_east() {
        let transform = GeoTransform::new(0.0, 0.0, 10.0, 90.0);
        let (top_latitude, top_longitude) = transform.coordinate_at_uv(0.5, 1.0);
        let (right_latitude, right_longitude) = transform.coordinate_at_uv(1.0, 0.5);
        assert!(top_longitude > 0.0);
        assert!(top_latitude.abs() < 0.000_001);
        assert!(right_latitude < 0.0);
        assert!(right_longitude.abs() < 0.000_001);
    }

    #[test]
    fn rotated_bounds_enclose_every_corner_across_the_date_line() {
        let transform = GeoTransform::new(0.0, 179.95, 20.0, 33.0);
        let bounds = transform.bounds();
        assert_eq!(bounds.split_at_antimeridian().len(), 2);
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            let (latitude, longitude) = transform.coordinate_at_uv(u, v);
            let longitude = unwrap_longitude(longitude, 179.95);
            assert!((bounds.south..=bounds.north).contains(&latitude));
            assert!((bounds.west..=bounds.east).contains(&longitude));
        }
    }
}
