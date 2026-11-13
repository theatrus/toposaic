use super::*;

const TILES: TileLayer<'static> = TileLayer::new("neighborhood-surfaces-v1", 13);

pub(super) fn paint(
    spec: &GenerationSpec,
    bounds: GeoBounds,
    cache_dir: &Path,
    field: &mut SurfaceField,
    cancellation: Option<&AtomicBool>,
) -> Result<usize> {
    let response = osm::fetch_tiled_response(
        cache_dir,
        TILES,
        LegacyCache::new(&[]),
        bounds,
        query,
        cancellation,
    )?;
    Ok(paint_elements(spec, field, response))
}

fn query(bounds: &[GeoBounds]) -> String {
    let mut selectors = String::new();
    for bounds in bounds {
        let bbox = format!(
            "({:.7},{:.7},{:.7},{:.7})",
            bounds.south, bounds.west, bounds.north, bounds.east
        );
        for selector in [
            "[\"landuse\"~\"^(grass|meadow|forest)$\"]",
            "[\"natural\"~\"^(grassland|wood)$\"]",
            "[\"leisure\"~\"^(pitch|track|playground|schoolyard)$\"]",
            "[\"amenity\"=\"parking\"]",
            "[\"highway\"=\"pedestrian\"][\"area\"=\"yes\"]",
        ] {
            selectors.push_str(&format!("way{selector}{bbox};relation{selector}{bbox};"));
        }
    }
    format!("[out:json][timeout:60];({selectors});out tags geom;")
}

fn class_for(tags: &HashMap<String, String>) -> Option<SurfaceClass> {
    let tag = |key: &str| tags.get(key).map(String::as_str).unwrap_or("");
    if tag("area") == "no"
        || !matches!(tag("building"), "" | "no")
        || matches!(tag("indoor"), "yes")
        || matches!(tag("location"), "underground" | "roof" | "rooftop")
        || matches!(tag("parking"), "underground" | "multi-storey" | "rooftop")
        || is_tunnel(tags)
    {
        return None;
    }
    // Facility boundaries do not describe a uniform physical surface.
    let leisure = tag("leisure");
    let parking =
        tag("amenity") == "parking" && !tags.contains_key("grades") && !tags.contains_key("grade");
    let pedestrian = tag("highway") == "pedestrian" && tag("area") == "yes";
    let specific_area =
        matches!(leisure, "pitch" | "track" | "playground" | "schoolyard") || parking || pedestrian;
    if specific_area {
        if tag("surface") == "grass" {
            return Some(SurfaceClass::Grass);
        }
        return match leisure {
            "pitch" | "track" => Some(SurfaceClass::Sport),
            "playground" => Some(SurfaceClass::Playground),
            _ => Some(SurfaceClass::Pavement),
        };
    }
    if matches!(tag("landuse"), "grass" | "meadow") || tag("natural") == "grassland" {
        return Some(SurfaceClass::Grass);
    }
    if tag("landuse") == "forest" || tag("natural") == "wood" {
        return Some(SurfaceClass::Forest);
    }
    None
}

struct Area {
    shell: Vec<[f32; 2]>,
    holes: Vec<Vec<[f32; 2]>>,
    class: SurfaceClass,
    id: u64,
}

fn areas_for(element: &OverpassWay, transform: GeoTransform, class: SurfaceClass) -> Vec<Area> {
    if element.element_type == "relation" {
        if element.tags.get("type").map(String::as_str) != Some("multipolygon") {
            return Vec::new();
        }
        let rings = normalized_relation_rings(element, transform);
        // Missing pieces of a hole must never turn into a filled playground.
        if rings.dropped > 0 {
            return Vec::new();
        }
        return rings
            .outer
            .into_iter()
            .map(|shell| {
                let holes = rings
                    .inner
                    .iter()
                    .filter(|hole| ring_contains(&shell, hole))
                    .cloned()
                    .collect();
                Area {
                    shell,
                    holes,
                    class,
                    id: element.id,
                }
            })
            .collect();
    }
    // A closed running-track way is often a centre line. Filling its oval
    // would color the infield too. Only explicitly mapped areas are usable.
    if element.tags.get("leisure").map(String::as_str) == Some("track")
        && element.tags.get("area").map(String::as_str) != Some("yes")
    {
        return Vec::new();
    }
    let Some(first) = element.geometry.first() else {
        return Vec::new();
    };
    let Some(last) = element.geometry.last() else {
        return Vec::new();
    };
    if element.geometry.len() < 4 || first.lat != last.lat || first.lon != last.lon {
        return Vec::new();
    }
    let mut shell = normalized_osm_points(element, transform);
    shell.pop();
    vec![Area {
        shell,
        holes: Vec::new(),
        class,
        id: element.id,
    }]
}

fn priority(class: SurfaceClass) -> u8 {
    match class {
        SurfaceClass::Forest | SurfaceClass::Grass => 0,
        SurfaceClass::Pavement => 1,
        SurfaceClass::Sport | SurfaceClass::Playground => 2,
        _ => 0,
    }
}

pub(super) fn paint_elements(
    spec: &GenerationSpec,
    field: &mut SurfaceField,
    response: OverpassResponse,
) -> usize {
    let transform = transform_for(spec);
    let mut areas: Vec<_> = response
        .elements
        .iter()
        .filter_map(|element| {
            class_for(&element.tags).map(|class| areas_for(element, transform, class))
        })
        .flatten()
        .collect();
    // Broad surfaces first, so a grass island inside pavement keeps its own
    // surface just as a small court inside a schoolyard does.
    areas.sort_by(|a, b| {
        area_size(&b.shell)
            .total_cmp(&area_size(&a.shell))
            .then_with(|| priority(a.class).cmp(&priority(b.class)))
            .then_with(|| a.id.cmp(&b.id))
    });
    for area in &areas {
        field.paint_surface_area_with_holes(&area.shell, &area.holes, area.class);
    }
    areas.len()
}

fn area_size(points: &[[f32; 2]]) -> f32 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f32>()
        .abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn way(spec: &GenerationSpec, entries: &[(&str, &str)], low: f64, high: f64) -> OverpassWay {
        let transform = transform_for(spec);
        let geometry = [
            (low, low),
            (high, low),
            (high, high),
            (low, high),
            (low, low),
        ]
        .map(|(u, v)| {
            let (lat, lon) = transform.coordinate_at_uv(u, v);
            OverpassPoint { lat, lon }
        })
        .to_vec();
        OverpassWay {
            id: 1,
            element_type: "way".into(),
            tags: tags(entries),
            geometry,
            ..Default::default()
        }
    }

    fn field() -> SurfaceField {
        SurfaceField::new(21, 21, vec![SurfaceClass::Rock; 441], "test").unwrap()
    }

    #[test]
    fn neighborhood_tags_distinguish_physical_and_activity_surfaces() {
        for (entries, expected) in [
            (vec![("landuse", "grass")], SurfaceClass::Grass),
            (vec![("natural", "wood")], SurfaceClass::Forest),
            (
                vec![("leisure", "pitch"), ("sport", "basketball")],
                SurfaceClass::Sport,
            ),
            (
                vec![("leisure", "pitch"), ("surface", "grass")],
                SurfaceClass::Grass,
            ),
            (vec![("leisure", "playground")], SurfaceClass::Playground),
            (vec![("leisure", "schoolyard")], SurfaceClass::Pavement),
            (vec![("amenity", "parking")], SurfaceClass::Pavement),
        ] {
            assert_eq!(class_for(&tags(&entries)), Some(expected));
        }
    }

    #[test]
    fn facility_boundaries_and_non_ground_areas_are_not_filled() {
        for entries in [
            vec![("leisure", "park")],
            vec![("amenity", "school")],
            vec![("amenity", "parking"), ("grades", "K;1;2;3;4;5")],
            vec![("amenity", "parking"), ("parking", "underground")],
            vec![("leisure", "pitch"), ("building", "yes")],
            vec![("leisure", "pitch"), ("indoor", "yes")],
        ] {
            assert_eq!(class_for(&tags(&entries)), None);
        }
    }

    #[test]
    fn smaller_courts_override_broad_pavement_regardless_of_input_order() {
        let spec = GenerationSpec::default();
        let court = way(&spec, &[("leisure", "pitch")], 0.3, 0.7);
        let pavement = way(&spec, &[("leisure", "schoolyard")], 0.1, 0.9);
        for elements in [vec![court.clone(), pavement.clone()], vec![pavement, court]] {
            let mut field = field();
            let count = paint_elements(
                &spec,
                &mut field,
                OverpassResponse {
                    elements,
                    remark: None,
                },
            );
            assert_eq!(count, 2);
            assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Sport);
            assert_eq!(field.class_at(0.2, 0.2), SurfaceClass::Pavement);
            field.paint_polyline(
                &[[0.1, 0.5], [0.9, 0.5]],
                spec.width_mm,
                1.0,
                SurfaceClass::Road,
            );
            assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Road);
            field.paint_building(&[[0.4, 0.4], [0.6, 0.4], [0.6, 0.6], [0.4, 0.6]], 8.0);
            assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Building);
        }
    }

    #[test]
    fn multipolygon_holes_survive_and_open_ways_are_skipped() {
        let spec = GenerationSpec::default();
        let shell = way(&spec, &[], 0.1, 0.9);
        let hole = way(&spec, &[], 0.4, 0.6);
        let relation = OverpassWay {
            element_type: "relation".into(),
            tags: tags(&[("type", "multipolygon"), ("leisure", "playground")]),
            members: vec![
                osm::OverpassMember {
                    member_type: "way".into(),
                    role: "outer".into(),
                    geometry: shell.geometry,
                    ..Default::default()
                },
                osm::OverpassMember {
                    member_type: "way".into(),
                    role: "inner".into(),
                    geometry: hole.geometry,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut open = way(&spec, &[("landuse", "grass")], 0.1, 0.9);
        open.geometry.pop();
        let mut field = field();
        assert_eq!(
            paint_elements(
                &spec,
                &mut field,
                OverpassResponse {
                    elements: vec![relation, open],
                    remark: None
                }
            ),
            1
        );
        assert_eq!(field.class_at(0.2, 0.2), SurfaceClass::Playground);
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Rock);
    }

    #[test]
    fn a_mapped_grass_island_inside_a_schoolyard_stays_grass() {
        let spec = GenerationSpec::default();
        let grass = way(&spec, &[("landuse", "grass")], 0.3, 0.7);
        let paved = way(&spec, &[("leisure", "schoolyard")], 0.1, 0.9);
        let mut field = field();
        paint_elements(
            &spec,
            &mut field,
            OverpassResponse {
                elements: vec![grass, paved],
                remark: None,
            },
        );
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Grass);
        assert_eq!(field.class_at(0.2, 0.2), SurfaceClass::Pavement);
    }

    #[test]
    fn query_does_not_request_whole_school_or_park_boundaries() {
        let q = query(&[bounds_for(&GenerationSpec::default())]);
        assert!(q.contains("schoolyard"));
        assert!(q.contains("playground"));
        assert!(!q.contains("[\"amenity\"=\"school\"]"));
        assert!(!q.contains("[\"leisure\"=\"park\"]"));
    }

    #[test]
    fn closed_track_lines_do_not_fill_the_infield() {
        let spec = GenerationSpec::default();
        for entries in [
            vec![("leisure", "track")],
            vec![("leisure", "track"), ("area", "no")],
            vec![("leisure", "pitch"), ("area", "no")],
        ] {
            let mut field = field();
            let response = OverpassResponse {
                elements: vec![way(&spec, &entries, 0.1, 0.9)],
                remark: None,
            };
            assert_eq!(paint_elements(&spec, &mut field, response), 0);
            assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Rock);
        }
        let mut field = field();
        let area = way(&spec, &[("leisure", "track"), ("area", "yes")], 0.1, 0.9);
        assert_eq!(
            paint_elements(
                &spec,
                &mut field,
                OverpassResponse {
                    elements: vec![area],
                    remark: None,
                }
            ),
            1
        );
        assert_eq!(field.class_at(0.5, 0.5), SurfaceClass::Sport);
    }
}
