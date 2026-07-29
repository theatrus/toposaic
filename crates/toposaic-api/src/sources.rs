//! Source bundles: the downloaded map data one setup needs, packed so the
//! same model can be built again on a machine with no network, or archived
//! against the day a provider retires a tile.
//!
//! A generation records every cache file it read or wrote (see
//! [`crate::cache::Recording`]), and that list is written beside the job's
//! print files. The bundle is built later, on request, because the files it
//! names can be large — one ESA WorldCover tile runs to tens of megabytes,
//! and no job should pay for an archive nobody asked for.

use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use toposaic_core::GenerationSpec;
use tracing::warn;
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::cache;

/// The recorded list, written into the job directory next to the print
/// files. Not an artifact: it is bookkeeping for building a bundle later,
/// and it would only clutter the download list.
pub const SOURCE_LIST_NAME: &str = "sources.json";

/// The built bundle's name inside the job directory. A job's file like any
/// other once it exists, so the browser download and the desktop save dialog
/// both reach it without knowing anything about bundles.
pub const BUNDLE_ARTIFACT_NAME: &str = "toposaic-sources.zip";

/// Names inside a bundle.
const BUNDLE_MANIFEST_NAME: &str = "toposaic-sources.json";
const BUNDLE_SETUP_NAME: &str = "toposaic-setup.json";
const BUNDLE_DATA_PREFIX: &str = "cache/";

/// Bumped when the layout changes in a way an older reader would get wrong.
/// An importer that meets a version it does not know refuses the file rather
/// than guessing at its shape.
const BUNDLE_VERSION: u32 = 1;

/// Cache subdirectories a bundle may carry, and so the only places an import
/// will write. Deliberately the same set the cache summary reports, minus
/// the place-search rows, which live in SQLite and describe someone's typing
/// rather than map data.
const BUNDLED_CATEGORIES: [&str; 5] = ["elevation", "world-cover", "osm", "imagery", "datum"];

/// A single import must not be able to fill the disk. The per-entry cap sits
/// above the largest thing a real bundle holds (a WorldCover tile), and the
/// total above a busy super-tile's whole source set.
const MAXIMUM_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAXIMUM_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAXIMUM_ENTRIES: usize = 100_000;

/// The cap on the upload itself, before anything is unpacked. Sits at the
/// unpacked total: the entries are stored, not deflated, so the two are
/// within a rounding error of each other.
pub const MAXIMUM_UPLOAD_BYTES: u64 = MAXIMUM_TOTAL_BYTES;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFile {
    /// Path relative to the map cache root, always with forward slashes so a
    /// bundle written on Windows imports on Linux.
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceList {
    pub files: Vec<SourceFile>,
    /// What the generating job said about its providers — names, licences,
    /// attribution. Kept here because the job row does not store it, and an
    /// archive without its attribution is of little use years later.
    #[serde(default)]
    pub data_sources: Vec<String>,
}

impl SourceList {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
}

/// What a bundle says about itself. The category tally is for a person
/// reading the archive years later: it names which providers the data came
/// from without their having to recognise the directory names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub version: u32,
    pub place_name: String,
    pub files: Vec<SourceFile>,
    pub categories: BTreeMap<String, u64>,
    /// Notes the generating job recorded about its data sources — provider
    /// names, licences, attribution.
    #[serde(default)]
    pub data_sources: Vec<String>,
}

/// Turns the recorded absolute paths into cache-relative entries with their
/// sizes. Files that vanished between generation and now are dropped with a
/// warning: a cache clear can run at any moment, and a half-listed bundle
/// beats refusing to write one.
pub fn source_list(
    log: &cache::SourceLog,
    map_cache_dir: &Path,
    data_sources: &[String],
) -> SourceList {
    let mut files = Vec::new();
    for path in log.paths() {
        let Some(relative) = cache_relative(path, map_cache_dir) else {
            warn!(
                path = %path.display(),
                "recorded source lies outside the map cache; leaving it out of the list"
            );
            continue;
        };
        match fs::metadata(path) {
            Ok(metadata) => files.push(SourceFile {
                path: relative,
                bytes: metadata.len(),
            }),
            Err(error) => warn!(
                %error,
                path = %path.display(),
                "recorded source is gone; leaving it out of the list"
            ),
        }
    }
    SourceList {
        files,
        data_sources: data_sources.to_vec(),
    }
}

/// Writes the list into a finished job's directory. Failure is reported to
/// the log and swallowed: a job that produced correct print files has
/// succeeded, whether or not it could also describe its inputs.
pub fn write_source_list(output_dir: &Path, map_cache_dir: &Path, data_sources: &[String]) {
    let list = source_list(&cache::current_sources(), map_cache_dir, data_sources);
    if list.files.is_empty() {
        return;
    }
    let path = output_dir.join(SOURCE_LIST_NAME);
    match serde_json::to_vec_pretty(&list) {
        Ok(bytes) => {
            if let Err(error) = fs::write(&path, bytes) {
                warn!(%error, path = %path.display(), "could not write the source list");
            }
        }
        Err(error) => warn!(%error, "could not encode the source list"),
    }
}

pub fn read_source_list(output_dir: &Path) -> Result<SourceList> {
    let path = output_dir.join(SOURCE_LIST_NAME);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "this job has no recorded source list ({}); only jobs generated \
             since source bundles arrived carry one",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes).context("parse the recorded source list")
}

/// Packs a job's recorded sources, its setup, and a manifest into a zip.
///
/// Stored without compression: every category is already compressed — PNG
/// and WebP tiles, deflated GeoTIFFs, the packed imagery raster — so
/// deflating again spends CPU on a rounding error. The JSON entries are
/// small enough not to change that.
pub fn build_bundle(
    list: &SourceList,
    spec: &GenerationSpec,
    map_cache_dir: &Path,
) -> Result<Vec<u8>> {
    if list.files.is_empty() {
        bail!("this job recorded no source files to bundle");
    }
    let mut buffer = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let mut packed = Vec::new();
    let mut categories: BTreeMap<String, u64> = BTreeMap::new();
    for file in &list.files {
        let absolute = map_cache_dir.join(&file.path);
        let bytes = match fs::read(&absolute) {
            Ok(bytes) => bytes,
            Err(error) => {
                // Same reasoning as the list: a cleared cache should give a
                // smaller bundle, not an error. The manifest describes what
                // is really inside, so the gap is visible.
                warn!(
                    %error,
                    path = %absolute.display(),
                    "source file is gone; leaving it out of the bundle"
                );
                continue;
            }
        };
        zip.start_file(format!("{BUNDLE_DATA_PREFIX}{}", file.path), options)?;
        zip.write_all(&bytes)?;
        *categories.entry(category_of(&file.path)).or_default() += 1;
        packed.push(SourceFile {
            path: file.path.clone(),
            bytes: bytes.len() as u64,
        });
    }
    if packed.is_empty() {
        bail!("every source file this job recorded has since been cleared from the cache");
    }

    let manifest = BundleManifest {
        version: BUNDLE_VERSION,
        place_name: spec.place_name.clone(),
        files: packed,
        categories,
        data_sources: list.data_sources.clone(),
    };
    zip.start_file(BUNDLE_MANIFEST_NAME, options)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    zip.start_file(BUNDLE_SETUP_NAME, options)?;
    zip.write_all(&serde_json::to_vec_pretty(spec)?)?;
    zip.finish()?;
    Ok(buffer)
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ImportReport {
    pub place_name: String,
    /// Files written into the cache by this import.
    pub added: u64,
    pub added_bytes: u64,
    /// Files the cache already held. `cache::store` never overwrites, so an
    /// existing entry always wins over the bundle's copy.
    pub already_present: u64,
    /// Entries refused: outside the known categories, or unreadable.
    pub rejected: u64,
}

/// Unpacks a bundle into the map cache so a later generation reads it
/// instead of the network, and returns the setup it carried.
///
/// Every entry is checked before anything is written. A zip is a file
/// someone else made, and this one is asked to place files by name — so
/// absolute paths, `..` traversal, and directories outside
/// [`BUNDLED_CATEGORIES`] are refused rather than sanitised, and the sizes
/// are capped so one import cannot fill the disk.
/// Takes any seekable reader rather than a byte slice: a real bundle
/// arrives as an upload of hundreds of megabytes, and the handler streams it
/// to a temporary file instead of holding it in memory.
pub fn import_bundle<R: Read + std::io::Seek>(
    reader: R,
    map_cache_dir: &Path,
) -> Result<(ImportReport, GenerationSpec)> {
    let mut archive = zip::ZipArchive::new(reader).context("read the source bundle archive")?;
    if archive.len() > MAXIMUM_ENTRIES {
        bail!(
            "this bundle holds {} entries, past the {MAXIMUM_ENTRIES} an import accepts",
            archive.len()
        );
    }

    let manifest: BundleManifest = {
        let mut entry = archive
            .by_name(BUNDLE_MANIFEST_NAME)
            .context("this file has no source-bundle manifest, so it is not a TopoSaic bundle")?;
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .context("read the bundle manifest")?;
        serde_json::from_str(&text).context("parse the bundle manifest")?
    };
    if manifest.version != BUNDLE_VERSION {
        bail!(
            "this bundle is version {}, and this build reads version {BUNDLE_VERSION}",
            manifest.version
        );
    }

    let spec: GenerationSpec = {
        let mut entry = archive
            .by_name(BUNDLE_SETUP_NAME)
            .context("this bundle carries no setup")?;
        let mut text = String::new();
        entry.read_to_string(&mut text).context("read the setup")?;
        serde_json::from_str(&text).context("parse the bundled setup")?
    };
    spec.validate()
        .map_err(|error| anyhow!("the bundled setup is not valid: {error}"))?;

    let mut report = ImportReport {
        place_name: manifest.place_name.clone(),
        ..ImportReport::default()
    };
    let mut total_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("read a bundle entry")?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let Some(relative) = name.strip_prefix(BUNDLE_DATA_PREFIX) else {
            // The manifest and the setup live at the root; anything else
            // there is not ours to place.
            if name != BUNDLE_MANIFEST_NAME && name != BUNDLE_SETUP_NAME {
                warn!(entry = %name, "bundle entry is outside the cache directory; skipped");
                report.rejected += 1;
            }
            continue;
        };
        let Some(target) = safe_cache_path(relative, map_cache_dir) else {
            warn!(entry = %name, "bundle entry is not a valid cache path; skipped");
            report.rejected += 1;
            continue;
        };
        if entry.size() > MAXIMUM_ENTRY_BYTES {
            warn!(entry = %name, bytes = entry.size(), "bundle entry is too large; skipped");
            report.rejected += 1;
            continue;
        }
        total_bytes += entry.size();
        if total_bytes > MAXIMUM_TOTAL_BYTES {
            bail!(
                "this bundle unpacks to more than {} GB, past what an import accepts",
                MAXIMUM_TOTAL_BYTES / (1024 * 1024 * 1024)
            );
        }
        // Decompress with the declared size as the ceiling, so a lying
        // header cannot make this read forever.
        let mut contents = Vec::new();
        entry
            .by_ref()
            .take(MAXIMUM_ENTRY_BYTES + 1)
            .read_to_end(&mut contents)
            .with_context(|| format!("unpack bundle entry {name}"))?;
        if contents.len() as u64 > MAXIMUM_ENTRY_BYTES {
            warn!(entry = %name, "bundle entry unpacked past its cap; skipped");
            report.rejected += 1;
            continue;
        }
        if target.is_file() {
            report.already_present += 1;
            continue;
        }
        cache::store(&target, &contents)
            .with_context(|| format!("write cache file {}", target.display()))?;
        report.added += 1;
        report.added_bytes += contents.len() as u64;
    }
    Ok((report, spec))
}

/// `path` as a forward-slash path relative to the cache root, or `None` when
/// it lies outside.
fn cache_relative(path: &Path, map_cache_dir: &Path) -> Option<String> {
    let relative = path.strip_prefix(map_cache_dir).ok()?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The absolute path a bundle entry may be written to, or `None` when the
/// name is not one an import is allowed to place.
///
/// Rejects, rather than repairs: an absolute path, any component that is not
/// a plain name (so `..` and `.` both go), a leading directory outside
/// [`BUNDLED_CATEGORIES`], and a name that reaches no file at all. Repairing
/// a hostile path tends to produce a different valid path, which is worse
/// than refusing it.
fn safe_cache_path(relative: &str, map_cache_dir: &Path) -> Option<PathBuf> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\\') {
        return None;
    }
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return None;
    }
    let mut parts = Vec::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            _ => return None,
        }
    }
    let (category, rest) = parts.split_first()?;
    if rest.is_empty() || !BUNDLED_CATEGORIES.contains(category) {
        return None;
    }
    let mut target = map_cache_dir.to_path_buf();
    for part in parts {
        target.push(part);
    }
    Some(target)
}

/// The cache category a relative path belongs to, for the manifest tally.
fn category_of(relative: &str) -> String {
    relative.split('/').next().unwrap_or("unknown").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temporary_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("toposaic-sources-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn sample_spec() -> GenerationSpec {
        GenerationSpec {
            place_name: "Mount Rainier".to_string(),
            ..GenerationSpec::default()
        }
    }

    #[test]
    fn a_bundle_round_trips_through_an_empty_cache() {
        let root = temporary_dir();
        let source_cache = root.join("source-cache");
        let target_cache = root.join("target-cache");
        write(&source_cache.join("elevation/8/1/2.png"), b"tile-bytes");
        write(
            &source_cache.join("osm/roads-v2-abc.json"),
            b"{\"elements\":[]}",
        );
        write(&source_cache.join("world-cover/tile-a.tif"), b"geotiff");

        let list = SourceList {
            files: vec![
                SourceFile {
                    path: "elevation/8/1/2.png".into(),
                    bytes: 10,
                },
                SourceFile {
                    path: "osm/roads-v2-abc.json".into(),
                    bytes: 15,
                },
                SourceFile {
                    path: "world-cover/tile-a.tif".into(),
                    bytes: 7,
                },
            ],
            data_sources: vec!["Mapzen Terrarium".to_string()],
        };
        let spec = sample_spec();
        let bundle = build_bundle(&list, &spec, &source_cache).unwrap();

        let (report, restored) = import_bundle(Cursor::new(&bundle), &target_cache).unwrap();
        assert_eq!(report.added, 3);
        assert_eq!(report.already_present, 0);
        assert_eq!(report.rejected, 0);
        assert_eq!(report.added_bytes, 10 + 15 + 7);
        assert_eq!(report.place_name, "Mount Rainier");
        assert_eq!(restored.place_name, spec.place_name);
        assert_eq!(restored.center_lat, spec.center_lat);
        assert_eq!(
            fs::read(target_cache.join("elevation/8/1/2.png")).unwrap(),
            b"tile-bytes"
        );
        assert_eq!(
            fs::read(target_cache.join("world-cover/tile-a.tif")).unwrap(),
            b"geotiff"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn importing_twice_keeps_what_the_cache_already_holds() {
        let root = temporary_dir();
        let source_cache = root.join("source-cache");
        let target_cache = root.join("target-cache");
        write(&source_cache.join("elevation/8/1/2.png"), b"fresh");
        write(&target_cache.join("elevation/8/1/2.png"), b"mine");
        let list = SourceList {
            files: vec![SourceFile {
                path: "elevation/8/1/2.png".into(),
                bytes: 5,
            }],
            data_sources: Vec::new(),
        };
        let bundle = build_bundle(&list, &sample_spec(), &source_cache).unwrap();

        let (report, _) = import_bundle(Cursor::new(&bundle), &target_cache).unwrap();
        assert_eq!(report.added, 0);
        assert_eq!(report.already_present, 1);
        assert_eq!(
            fs::read(target_cache.join("elevation/8/1/2.png")).unwrap(),
            b"mine",
            "an import never overwrites a cache entry"
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hostile_entry_names_are_refused_rather_than_repaired() {
        let cache = Path::new("/tmp/toposaic-cache");
        for name in [
            "../outside.png",
            "elevation/../../outside.png",
            "/etc/passwd",
            "",
            "elevation",
            "jobs/model.3mf",
            "places/query.json",
            "unknown/thing.bin",
            "./elevation/8/1/2.png",
            "elevation\\8\\1\\2.png",
        ] {
            assert!(
                safe_cache_path(name, cache).is_none(),
                "{name} should be refused"
            );
        }
        for name in [
            "elevation/8/1/2.png",
            "elevation/mapterhorn/8/1/2.webp",
            "world-cover/tile-a.tif",
            "osm/roads-v2-a.json",
            "imagery/s2rgbnir-a.bin",
            "datum/coops-stations-v1.json",
        ] {
            assert_eq!(
                safe_cache_path(name, cache),
                Some(cache.join(name)),
                "{name} should be accepted"
            );
        }
    }

    #[test]
    fn a_traversal_entry_writes_nothing_outside_the_cache() {
        let root = temporary_dir();
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            let manifest = BundleManifest {
                version: BUNDLE_VERSION,
                place_name: "Somewhere".into(),
                files: Vec::new(),
                categories: BTreeMap::new(),
                data_sources: Vec::new(),
            };
            zip.start_file(BUNDLE_MANIFEST_NAME, options).unwrap();
            zip.write_all(&serde_json::to_vec(&manifest).unwrap())
                .unwrap();
            zip.start_file(BUNDLE_SETUP_NAME, options).unwrap();
            zip.write_all(&serde_json::to_vec(&sample_spec()).unwrap())
                .unwrap();
            // zip's writer refuses some of these itself, so the escape is
            // spelled inside an accepted prefix.
            zip.start_file(format!("{BUNDLE_DATA_PREFIX}../escaped.png"), options)
                .unwrap();
            zip.write_all(b"nope").unwrap();
            zip.start_file(format!("{BUNDLE_DATA_PREFIX}places/typing.json"), options)
                .unwrap();
            zip.write_all(b"nope").unwrap();
            zip.finish().unwrap();
        }

        let (report, _) = import_bundle(Cursor::new(&buffer), &cache).unwrap();
        assert_eq!(report.added, 0);
        assert_eq!(report.rejected, 2);
        assert!(!root.join("escaped.png").exists());
        assert!(!cache.join("places").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_that_is_not_a_bundle_is_refused_clearly() {
        let cache = temporary_dir();
        let error = import_bundle(Cursor::new(b"not a zip at all"), &cache).unwrap_err();
        assert!(format!("{error:#}").contains("archive"), "{error:#}");

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            zip.start_file("readme.txt", SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"hello").unwrap();
            zip.finish().unwrap();
        }
        let error = import_bundle(Cursor::new(&buffer), &cache).unwrap_err();
        assert!(
            format!("{error:#}").contains("not a TopoSaic bundle"),
            "{error:#}"
        );

        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn a_newer_bundle_version_is_refused_instead_of_guessed_at() {
        let cache = temporary_dir();
        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default();
            zip.start_file(BUNDLE_MANIFEST_NAME, options).unwrap();
            zip.write_all(
                serde_json::json!({
                    "version": BUNDLE_VERSION + 1,
                    "place_name": "Later",
                    "files": [],
                    "categories": {},
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
            zip.finish().unwrap();
        }
        let error = import_bundle(Cursor::new(&buffer), &cache).unwrap_err();
        assert!(format!("{error:#}").contains("version"), "{error:#}");

        fs::remove_dir_all(cache).unwrap();
    }

    #[test]
    fn the_list_skips_files_outside_the_cache_and_files_that_vanished() {
        let root = temporary_dir();
        let cache = root.join("cache");
        let present = cache.join("elevation/8/1/2.png");
        write(&present, b"tile");

        let _recording = cache::Recording::begin();
        cache::note(&present);
        cache::note(&cache.join("elevation/8/1/gone.png"));
        cache::note(&root.join("elsewhere/secret.txt"));
        let list = source_list(&cache::current_sources(), &cache, &[]);

        assert_eq!(
            list.files,
            [SourceFile {
                path: "elevation/8/1/2.png".into(),
                bytes: 4,
            }]
        );
        assert_eq!(list.total_bytes(), 4);

        fs::remove_dir_all(root).unwrap();
    }
}
