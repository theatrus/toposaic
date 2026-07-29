#[doc(hidden)]
pub mod analysis;
mod export;
mod geography;
mod heightfield;
mod jigsaw;
mod marine;
mod marker;
mod mesh;
mod mount;
mod mount_layout;
mod piece;
mod planar_mesh;
mod preview;
mod project;
mod spec;
mod surface;
mod text;
mod tray;

pub use geography::{GeoBounds, GeoTransform, normalize_longitude};
pub use heightfield::{DespikeReport, HeightField, VerticalReference};
pub use marine::{
    MarineOutcome, ResolvedMarineLevel, apply_flat_marine_surface, resolve_marine_level,
};
pub use preview::build_height_preview;
pub use project::{
    Artifact, ProjectManifest, artifact_path, generate_marker_artifacts, generate_project,
    generate_project_with_fields, generate_project_with_fields_cancellable,
    generate_project_with_height_field, generate_tray_artifacts, generate_wall_mount_artifacts,
};
pub use spec::{
    AerialStyle, BorderSpec, BridgeStructure, BuildingSpec, ClassBorders, ColorOutputSpec,
    DotMarkerStyle, ElevationSource, FerryStyle, FlagMarkerStyle, GenerationSpec, LabelFont,
    LineScaleSpec, LineStyle, MapFrame, MapLabelStyle, MapMarker, MarineGeometry, MarineLevel,
    MarineSpec, MarkerKind, MarkerSpec, PuzzleRetentionSpec, RailLifecycle, RailStyle,
    ResolvedRoadDetail, RoadDetail, SlopeGateSpec, SteepForestTarget, SuperTileAnchor,
    SurfaceClass, ThreeMfStyle, TrailRoute, TrayLabelFont, TrayLabelPosition, TraySpec,
    WallMountSpec, WallMountStyle, WallMountTarget,
};
pub use surface::{NativeClassGrid, SlopeGateDemotion, SlopeGates, SurfaceField};
