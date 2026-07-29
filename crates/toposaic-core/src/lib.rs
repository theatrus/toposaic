#[doc(hidden)]
pub mod analysis;
mod coastline;
mod export;
mod geography;
mod heightfield;
mod jigsaw;
mod marine;
mod marker;
mod mesh;
mod mount;
mod mount_layout;
mod palette;
mod piece;
mod planar_mesh;
mod preview;
mod project;
mod spec;
mod surface;
mod text;
mod tray;

pub use coastline::{OceanExtent, assemble_ocean};
pub use geography::{GeoBounds, GeoTransform, normalize_longitude};
pub use heightfield::{
    DespikeReport, HeightField, HeightFrame, VerticalReference, exaggeration_for_metres_per_mm,
    height_frame_for_bounds, metres_per_mm_for_exaggeration, resolve_height_frame,
};
pub use marine::{
    MarineOutcome, ResolvedMarineLevel, TidalOffsets, apply_flat_marine_surface,
    resolve_marine_level,
};
pub use palette::{
    GroundImagery, GroundPalette, GroundPaletteEntry, GroundPaletteOptions,
    MAXIMUM_PALETTE_ENTRIES, NO_GROUND_MATERIAL, assign_locked_palette, discover_ground_palette,
};
pub use preview::build_height_preview;
pub use project::{
    Artifact, ProjectManifest, artifact_path, generate_marker_artifacts, generate_project,
    generate_project_with_fields, generate_project_with_fields_cancellable,
    generate_project_with_height_field, generate_tray_artifacts, generate_wall_mount_artifacts,
};
pub use spec::{
    AerialStyle, BorderSpec, BridgeStructure, BuildingSpec, ClassBorders, ColorOutputSpec,
    DatumReference, DotMarkerStyle, ElevationSource, FerryStyle, FlagMarkerStyle, GenerationSpec,
    GroundColorMode, GroundPaletteSpec, HeightMode, HeightScaleSpec, LabelFont, LineScaleSpec,
    LineStyle, MapFrame, MapLabelStyle, MapMarker, MarineGeometry, MarineLevel, MarineSpec,
    MarkerKind, MarkerSpec, PuzzleRetentionSpec, RailLifecycle, RailStyle, ResolvedRoadDetail,
    RoadDetail, SlopeGateSpec, SteepForestTarget, SuperTileAnchor, SurfaceClass, ThreeMfStyle,
    TrailRoute, TrayLabelFont, TrayLabelPosition, TraySpec, WallMountSpec, WallMountStyle,
    WallMountTarget,
};
pub use surface::{NativeClassGrid, SlopeGateDemotion, SlopeGates, SurfaceField};
