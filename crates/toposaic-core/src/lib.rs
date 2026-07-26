#[doc(hidden)]
pub mod analysis;
mod export;
mod heightfield;
mod jigsaw;
mod mesh;
mod mount;
mod piece;
mod preview;
mod project;
mod spec;
mod surface;
mod text;
mod tray;

pub use heightfield::{DespikeReport, HeightField};
pub use preview::build_height_preview;
pub use project::{
    Artifact, ProjectManifest, artifact_path, generate_project, generate_project_with_fields,
    generate_project_with_fields_cancellable, generate_project_with_height_field,
    generate_tray_artifacts,
};
pub use spec::{
    AerialStyle, BorderSpec, BridgeStructure, BuildingSpec, ClassBorders, ColorOutputSpec,
    ElevationSource, GenerationSpec, LineStyle, RailLifecycle, RailStyle, ResolvedRoadDetail,
    RoadDetail, SlopeGateSpec, SteepForestTarget, SuperTileAnchor, SurfaceClass, ThreeMfStyle,
    TrailRoute, TraySpec, WallMountSpec, WallMountStyle, WallMountTarget,
};
pub use surface::{NativeClassGrid, SlopeGateDemotion, SlopeGates, SurfaceField};
