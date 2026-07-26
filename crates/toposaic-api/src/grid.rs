//! Tile-grid geometry helpers used by the adjacent-grid job runner.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use toposaic_core::{Artifact, GenerationSpec, HeightField, SuperTileAnchor};

use crate::geo;

pub(crate) fn adjacent_tile_specs(spec: &GenerationSpec) -> Vec<GenerationSpec> {
    let row_anchor = match spec.super_tile_anchor {
        SuperTileAnchor::TopLeft => 0.0,
        SuperTileAnchor::Center => (f64::from(spec.adjacent_rows) - 1.0) / 2.0,
    };
    let column_anchor = match spec.super_tile_anchor {
        SuperTileAnchor::TopLeft => 0.0,
        SuperTileAnchor::Center => (f64::from(spec.adjacent_columns) - 1.0) / 2.0,
    };
    (0..spec.adjacent_rows)
        .flat_map(|row| {
            (0..spec.adjacent_columns).map(move |column| {
                let mut tile = spec.clone();
                let row_offset = f64::from(row) - row_anchor;
                let column_offset = f64::from(column) - column_anchor;
                (tile.center_lat, tile.center_lon) = geo::offset_coordinates(
                    spec.center_lat,
                    spec.center_lon,
                    -row_offset * spec.ground_span_km,
                    column_offset * spec.ground_span_km,
                );
                tile.adjacent_tile_column = column;
                tile.adjacent_tile_row = row;
                tile
            })
        })
        .collect()
}

pub(crate) fn mosaic_tray_spec(spec: &GenerationSpec) -> GenerationSpec {
    let mut mosaic = spec.clone();
    mosaic.width_mm *= spec.adjacent_columns as f32;
    mosaic.rows *= spec.adjacent_rows;
    mosaic.columns *= spec.adjacent_columns;
    mosaic.ground_span_km *= spec.adjacent_columns as f64;
    mosaic.adjacent_tile_column = 0;
    mosaic.adjacent_tile_row = 0;
    mosaic.tray.individual_tiles = false;
    mosaic.tray.segment_columns = spec.adjacent_columns;
    mosaic.tray.segment_rows = spec.adjacent_rows;
    mosaic
}

pub(crate) fn stitch_height_fields(
    fields: &[HeightField],
    rows: u32,
    columns: u32,
) -> Result<HeightField> {
    if rows == 0 || columns == 0 || fields.len() != (rows * columns) as usize {
        bail!("height fields do not match the adjacent tray grid");
    }
    let tile_width = fields[0].width;
    let tile_height = fields[0].height;
    if fields
        .iter()
        .any(|field| field.width != tile_width || field.height != tile_height)
    {
        bail!("adjacent height fields must use matching sample dimensions");
    }
    let width = columns as usize * (tile_width - 1) + 1;
    let height = rows as usize * (tile_height - 1) + 1;
    let mut sums = vec![0.0_f32; width * height];
    let mut counts = vec![0_u8; width * height];
    for (tile_index, field) in fields.iter().enumerate() {
        let tile_row = tile_index / columns as usize;
        let tile_column = tile_index % columns as usize;
        let x_offset = tile_column * (tile_width - 1);
        // Tile specs run north to south (tile row 0 is the northmost tile)
        // while height-field rows run south to north (row 0 is the southmost
        // sample), so the northmost tile lands at the top of the stitched
        // field, not at y offset 0.
        let y_offset = (rows as usize - 1 - tile_row) * (tile_height - 1);
        for y in 0..tile_height {
            for x in 0..tile_width {
                let output = (y_offset + y) * width + x_offset + x;
                sums[output] += field.values_m[y * tile_width + x];
                counts[output] += 1;
            }
        }
    }
    for (value, count) in sums.iter_mut().zip(counts) {
        *value /= f32::from(count);
    }
    HeightField::new(width, height, sums, "stitched adjacent elevation grid")
}

pub(crate) fn copy_grid_artifact(
    source: &Path,
    destination: &Path,
    name: &str,
    media_type: &str,
    artifacts: &mut Vec<Artifact>,
) -> Result<()> {
    fs::copy(source, destination)
        .with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
    artifacts.push(local_artifact(destination, name, media_type)?);
    Ok(())
}

pub(crate) fn local_artifact(path: &Path, name: &str, media_type: &str) -> Result<Artifact> {
    Ok(Artifact {
        name: name.to_owned(),
        media_type: media_type.to_owned(),
        bytes: fs::metadata(path)?.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn super_tile_grid_uses_the_current_tile_as_its_top_left_anchor() {
        let spec = GenerationSpec {
            center_lat: 46.0,
            center_lon: -121.0,
            ground_span_km: 10.0,
            adjacent_columns: 3,
            adjacent_rows: 2,
            ..GenerationSpec::default()
        };
        let tiles = adjacent_tile_specs(&spec);

        assert_eq!(tiles.len(), 6);
        assert_eq!(tiles[0].center_lat, spec.center_lat);
        assert_eq!(tiles[0].center_lon, spec.center_lon);
        assert!(tiles[1].center_lon > tiles[0].center_lon);
        assert!(tiles[3].center_lat < tiles[0].center_lat);
        assert_eq!(tiles[5].adjacent_tile_column, 2);
        assert_eq!(tiles[5].adjacent_tile_row, 1);
    }

    #[test]
    fn super_tile_grid_can_use_the_selected_point_as_its_center() {
        let spec = GenerationSpec {
            center_lat: 46.0,
            center_lon: -121.0,
            ground_span_km: 10.0,
            adjacent_columns: 5,
            adjacent_rows: 3,
            super_tile_anchor: SuperTileAnchor::Center,
            ..GenerationSpec::default()
        };
        let tiles = adjacent_tile_specs(&spec);
        let top_left = &tiles[0];
        let center = &tiles[7];
        let bottom_right = &tiles[14];

        assert_eq!(tiles.len(), 15);
        assert!(top_left.center_lat > spec.center_lat);
        assert!(top_left.center_lon < spec.center_lon);
        assert_eq!(center.center_lat, spec.center_lat);
        assert_eq!(center.center_lon, spec.center_lon);
        assert!(bottom_right.center_lat < spec.center_lat);
        assert!(bottom_right.center_lon > spec.center_lon);
        assert!(
            ((top_left.center_lat + bottom_right.center_lat) / 2.0 - spec.center_lat).abs() < 1e-9
        );
        assert!(
            ((top_left.center_lon + bottom_right.center_lon) / 2.0 - spec.center_lon).abs() < 1e-9
        );
    }

    #[test]
    fn mosaic_tray_follows_the_adjacent_tile_grid() {
        let spec = GenerationSpec {
            width_mm: 100.0,
            rows: 4,
            columns: 5,
            adjacent_columns: 3,
            adjacent_rows: 2,
            adjacent_interlocks: true,
            ..GenerationSpec::default()
        };
        let tray = mosaic_tray_spec(&spec);

        assert_eq!(tray.width_mm, 300.0);
        assert_eq!(tray.rows, 8);
        assert_eq!(tray.columns, 15);
        assert_eq!(tray.tray.segment_rows, 2);
        assert_eq!(tray.tray.segment_columns, 3);
        assert!(tray.adjacent_interlocks);
    }

    #[test]
    fn stitched_tray_height_field_averages_shared_samples() {
        let left = HeightField::new(2, 2, vec![1.0, 2.0, 3.0, 4.0], "left").unwrap();
        let right = HeightField::new(2, 2, vec![4.0, 5.0, 6.0, 7.0], "right").unwrap();
        let stitched = stitch_height_fields(&[left, right], 1, 2).unwrap();

        assert_eq!((stitched.width, stitched.height), (3, 2));
        assert_eq!(stitched.values_m, vec![1.0, 3.0, 5.0, 3.0, 5.0, 7.0]);
    }

    #[test]
    fn vertical_stitch_pairs_the_north_tile_bottom_with_the_south_tile_top() {
        // Tile row 0 is the NORTH tile; height-field row 0 is the SOUTH
        // sample row. The shared seam therefore averages the north tile's
        // row 0 (its southern edge) with the south tile's row h-1 (its
        // northern edge).
        let north = HeightField::new(2, 2, vec![10.0, 11.0, 20.0, 21.0], "north").unwrap();
        let south = HeightField::new(2, 2, vec![0.0, 1.0, 8.0, 9.0], "south").unwrap();
        let stitched = stitch_height_fields(&[north, south], 2, 1).unwrap();

        assert_eq!((stitched.width, stitched.height), (2, 3));
        // Row 0 (southmost) is the south tile's own southern edge.
        assert_eq!(&stitched.values_m[0..2], &[0.0, 1.0]);
        // The seam row averages south row 1 (8, 9) with north row 0 (10, 11).
        assert_eq!(&stitched.values_m[2..4], &[9.0, 10.0]);
        // Row 2 (northmost) is the north tile's own northern edge.
        assert_eq!(&stitched.values_m[4..6], &[20.0, 21.0]);
    }
}
