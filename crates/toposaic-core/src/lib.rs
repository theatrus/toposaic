#[doc(hidden)]
pub mod analysis;
mod export;
mod heightfield;
mod jigsaw;
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

pub use heightfield::{DespikeReport, HeightField};
pub use preview::build_height_preview;
pub use project::{
    Artifact, ProjectManifest, artifact_path, generate_marker_artifacts, generate_project,
    generate_project_with_fields, generate_project_with_fields_cancellable,
    generate_project_with_height_field, generate_tray_artifacts, generate_wall_mount_artifacts,
};
pub use spec::{
    AerialStyle, BorderSpec, BridgeStructure, BuildingSpec, ClassBorders, ColorOutputSpec,
    ElevationSource, GenerationSpec, LabelFont, LineScaleSpec, LineStyle, MapLabelStyle, MapMarker,
    MarkerKind, MarkerSpec, PuzzleRetentionSpec, RailLifecycle, RailStyle, ResolvedRoadDetail,
    RoadDetail, SlopeGateSpec, SteepForestTarget, SuperTileAnchor, SurfaceClass, ThreeMfStyle,
    TrailRoute, TrayLabelFont, TrayLabelPosition, TraySpec, WallMountSpec, WallMountStyle,
    WallMountTarget,
};
pub use surface::{NativeClassGrid, SlopeGateDemotion, SlopeGates, SurfaceField};
