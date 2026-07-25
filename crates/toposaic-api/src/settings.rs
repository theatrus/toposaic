use std::{env, ffi::OsString, path::PathBuf};

const LEGACY_DATA_DIR: &str = "TERRAIN_DATA_DIR";
const LEGACY_BIND: &str = "TERRAIN_BIND";
const LEGACY_CACHE_DIR: &str = "TERRAIN_CACHE_DIR";
const LEGACY_GEOCODER_URL: &str = "NOMINATIM_BASE_URL";

pub fn data_dir() -> PathBuf {
    env_path("TOPOSAIC_DATA_DIR", LEGACY_DATA_DIR).unwrap_or_else(|| PathBuf::from("data"))
}

pub fn bind_address() -> String {
    env_string("TOPOSAIC_BIND", LEGACY_BIND).unwrap_or_else(|| "127.0.0.1:8787".into())
}

pub fn cache_dir_override() -> Option<PathBuf> {
    env_path("TOPOSAIC_CACHE_DIR", LEGACY_CACHE_DIR)
}

pub fn geocoder_base_url() -> String {
    env_string("TOPOSAIC_GEOCODER_URL", LEGACY_GEOCODER_URL)
        .unwrap_or_else(|| "https://nominatim.openstreetmap.org".into())
}

pub fn allowed_origins() -> Vec<String> {
    env::var("TOPOSAIC_ALLOWED_ORIGINS")
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn env_path(name: &str, legacy_name: &str) -> Option<PathBuf> {
    env_value(name)
        .or_else(|| env_value(legacy_name))
        .map(PathBuf::from)
}

fn env_string(name: &str, legacy_name: &str) -> Option<String> {
    env_value(name)
        .or_else(|| env_value(legacy_name))
        .and_then(|value| value.into_string().ok())
}

fn env_value(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}
