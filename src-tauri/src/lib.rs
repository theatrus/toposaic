use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

static ENGINE_STARTED: OnceLock<()> = OnceLock::new();

fn app_data_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("Could not find the TopoSaic data folder: {error}"))
}

fn job_dir(data_dir: &Path, job_id: &str) -> Result<PathBuf, String> {
    let job_id = Uuid::parse_str(job_id)
        .map_err(|_| "The job ID is not valid.".to_owned())?
        .hyphenated()
        .to_string();
    Ok(data_dir.join("jobs").join(job_id))
}

fn source_artifact_path(
    data_dir: &Path,
    job_id: &str,
    artifact_name: &str,
) -> Result<PathBuf, String> {
    let output_dir = job_dir(data_dir, job_id)?;
    toposaic_core::artifact_path(&output_dir, artifact_name)
        .ok_or_else(|| "The requested print file does not exist.".to_owned())
}

/// A file-system-safe folder name built from the place the model is of.
/// Runs of anything that is not a letter or a digit become one dash, so a
/// name the user typed cannot walk out of the folder they chose.
fn folder_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        return "toposaic".to_owned();
    }
    // Long place names exist; keep the folder name workable on every OS.
    // Counted in CHARACTERS: `String::truncate` takes a byte length and
    // panics when that lands inside a character, which a place name in any
    // non-Latin script will do.
    slug.chars()
        .take(48)
        .collect::<String>()
        .trim_end_matches('-')
        .to_owned()
}

/// A folder inside `parent` that does not exist yet, starting from `stem`
/// and counting up. Saving a job never writes over an earlier one, which a
/// plain copy into the chosen folder would do silently for every file whose
/// name a previous run also used.
fn free_directory(parent: &Path, stem: &str) -> Result<PathBuf, String> {
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            stem.to_owned()
        } else {
            format!("{stem}-{suffix}")
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Could not find a free folder name.".to_owned())
}

/// Name of the setup a folder save writes beside the print files.
const SETUP_FILE_NAME: &str = "toposaic-setup.json";
/// Version of the setup-export shape. Mirrors `SETUPS_EXPORT_VERSION` in
/// app/terrain/studio.tsx, which is what reads a file back.
const SETUP_EXPORT_VERSION: u32 = 1;

/// The setup document for a finished job, in the shape the app's setup
/// import reads: one named setup carrying the spec.
///
/// The spec comes from the job's OWN manifest, not from whatever the editor
/// holds now. The point of saving it beside the files is to keep the setup
/// that made THEM, and the editor has usually moved on by the time anyone
/// saves — a slider nudged after generating would otherwise be written down
/// as the setup that produced the print.
fn setup_document(manifest_path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|error| format!("Could not read the job manifest: {error}"))?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("Could not read the job manifest: {error}"))?;
    let spec = manifest
        .get("spec")
        .ok_or_else(|| "The job manifest carries no model setup.".to_owned())?;
    let name = spec
        .get("place_name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("TopoSaic setup");
    serde_json::to_string_pretty(&serde_json::json!({
        "version": SETUP_EXPORT_VERSION,
        "setups": [{ "name": name, "spec": spec }],
    }))
    .map_err(|error| format!("Could not write the setup: {error}"))
}

/// Copies the NAMED artifacts of a finished job into a new folder under
/// `destination`. Returns the folder and how many files landed in it.
///
/// The caller passes the list rather than this sweeping the job folder,
/// because not everything in there is something to hand over: `preview.json`
/// is the app's own preview data and appears nowhere in the download list,
/// and the per-piece STLs are an alternative to the combined 3MF rather than
/// a companion to it. Each name is resolved through `artifact_path`, so a
/// name cannot reach outside the job folder.
async fn copy_job_artifacts(
    source_dir: &Path,
    destination: &Path,
    folder_name: &str,
    artifact_names: &[String],
) -> Result<(PathBuf, usize), String> {
    if !destination.is_dir() {
        return Err("The selected folder does not exist.".to_owned());
    }
    if artifact_names.is_empty() {
        return Err("There are no files to save.".to_owned());
    }
    let sources = artifact_names
        .iter()
        .map(|name| {
            toposaic_core::artifact_path(source_dir, name)
                .map(|path| (name, path))
                .ok_or_else(|| format!("{name} is no longer on disk."))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let target = free_directory(destination, &folder_slug(folder_name))?;
    tokio::fs::create_dir(&target)
        .await
        .map_err(|error| format!("Could not create {}: {error}", target.display()))?;

    let mut copied = 0;
    for (name, source) in sources {
        if let Err(error) = tokio::fs::copy(source, target.join(name)).await {
            // Take the folder with it. A run that stops halfway — a full
            // disk, a pulled drive — otherwise leaves a folder holding part
            // of a job, which looks exactly like a finished one.
            let _ = tokio::fs::remove_dir_all(&target).await;
            return Err(format!("Could not save {name}: {error}"));
        }
        copied += 1;
    }

    // And the setup that made them, so a folder of prints can be turned
    // back into the model that produced it. A job with no manifest to read
    // is not worth failing a good save over — the files are already there.
    let manifest_path = source_dir.join("manifest.json");
    if manifest_path.is_file() {
        match setup_document(&manifest_path) {
            Ok(document) => {
                if let Err(error) = tokio::fs::write(target.join(SETUP_FILE_NAME), document).await {
                    let _ = tokio::fs::remove_dir_all(&target).await;
                    return Err(format!("Could not save {SETUP_FILE_NAME}: {error}"));
                }
                copied += 1;
            }
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&target).await;
                return Err(error);
            }
        }
    }
    Ok((target, copied))
}

#[tauri::command]
async fn save_artifact(
    app: tauri::AppHandle,
    job_id: String,
    artifact_name: String,
) -> Result<Option<u64>, String> {
    let data_dir = app_data_dir(&app)?;
    let source = source_artifact_path(&data_dir, &job_id, &artifact_name)?;
    let extension = Path::new(&artifact_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_owned);
    let mut dialog = app
        .dialog()
        .file()
        .set_title(format!("Save {artifact_name}"))
        .set_file_name(&artifact_name);
    if let Some(extension) = extension.as_deref() {
        dialog = dialog.add_filter(extension.to_uppercase(), &[extension]);
    }
    let Some(destination) = dialog.blocking_save_file() else {
        return Ok(None);
    };
    let destination = destination
        .into_path()
        .map_err(|error| format!("The selected file path is not valid: {error}"))?;
    if !destination.parent().is_some_and(|parent| parent.is_dir()) {
        return Err("The selected folder does not exist.".to_owned());
    }

    tokio::fs::copy(source, destination)
        .await
        .map(Some)
        .map_err(|error| format!("Could not save {artifact_name}: {error}"))
}

/// What one "save all" wrote, for the message the app shows afterwards.
#[derive(serde::Serialize)]
struct SavedFolder {
    directory: String,
    files: usize,
}

/// Saves a set of a job's files in one go: one folder picker, one copy,
/// rather than a save dialog each. The front end names the set — the print
/// files, or the STLs.
#[tauri::command]
async fn save_all_artifacts(
    app: tauri::AppHandle,
    job_id: String,
    folder_name: String,
    artifact_names: Vec<String>,
) -> Result<Option<SavedFolder>, String> {
    let data_dir = app_data_dir(&app)?;
    let source_dir = job_dir(&data_dir, &job_id)?;
    if !source_dir.is_dir() {
        return Err("That job's print files are no longer on disk.".to_owned());
    }

    let Some(destination) = app
        .dialog()
        .file()
        .set_title("Save every print file to a folder")
        .blocking_pick_folder()
    else {
        return Ok(None);
    };
    let destination = destination
        .into_path()
        .map_err(|error| format!("The selected folder is not valid: {error}"))?;

    let (directory, files) =
        copy_job_artifacts(&source_dir, &destination, &folder_name, &artifact_names).await?;
    Ok(Some(SavedFolder {
        directory: directory.display().to_string(),
        files,
    }))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![save_artifact, save_all_artifacts])
        .setup(|app| {
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_process::init())?;
                if let Err(error) = app
                    .handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())
                {
                    eprintln!("Updater checks are unavailable: {error}");
                }
            }

            let app_handle = app.handle().clone();
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            ENGINE_STARTED.get_or_init(|| {
                tauri::async_runtime::spawn(async move {
                    if let Err(error) =
                        toposaic_api::run_with(data_dir, "127.0.0.1:38787".into()).await
                    {
                        eprintln!("terrain engine stopped: {error:#}");
                        app_handle.exit(1);
                    }
                });
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running TopoSaic");
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn desktop_save_only_reads_named_artifacts_from_uuid_job_folders() {
        let root = std::env::temp_dir().join(format!("toposaic-save-{}", Uuid::new_v4()));
        let job_id = Uuid::new_v4();
        let job_dir = root.join("jobs").join(job_id.hyphenated().to_string());
        fs::create_dir_all(&job_dir).unwrap();
        let artifact = job_dir.join("terrain.3mf");
        fs::write(&artifact, b"3MF").unwrap();

        assert_eq!(
            source_artifact_path(&root, &job_id.to_string(), "terrain.3mf").unwrap(),
            artifact
        );
        assert!(source_artifact_path(&root, "not-a-uuid", "terrain.3mf").is_err());
        assert!(source_artifact_path(&root, &job_id.to_string(), "../terrain.3mf").is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn folder_names_stay_inside_the_folder_the_user_picked() {
        assert_eq!(folder_slug("Mount Rainier"), "Mount-Rainier");
        assert_eq!(folder_slug("  Zürich / Üetliberg  "), "Zürich-Üetliberg");
        assert_eq!(folder_slug("富士山"), "富士山");
        // A name that would otherwise walk out of the chosen folder, and
        // one that would leave nothing behind at all.
        assert_eq!(folder_slug("../../etc"), "etc");
        assert_eq!(folder_slug("/"), "toposaic");
        assert_eq!(folder_slug(""), "toposaic");
        assert!(!folder_slug(&"x".repeat(200)).contains('/'));
        assert_eq!(folder_slug(&"x".repeat(200)).chars().count(), 48);
        // Cutting a long name to length must count characters, not bytes:
        // this name puts a byte-48 cut inside a character.
        let long_kanji = format!("a{}", "富".repeat(60));
        assert_eq!(folder_slug(&long_kanji).chars().count(), 48);
    }

    /// A folder of prints can be turned back into the model that made it.
    #[tokio::test]
    async fn a_saved_folder_carries_the_setup_that_made_it() {
        let root = std::env::temp_dir().join(format!("toposaic-setup-{}", Uuid::new_v4()));
        let source = root.join("job");
        let destination = root.join("picked");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("toposaic.3mf"), b"3MF").unwrap();
        fs::write(
            source.join("manifest.json"),
            br#"{"generator":"toposaic/0.5.0","spec":{"place_name":" Mount Rainier ","rows":3}}"#,
        )
        .unwrap();

        let (folder, files) = copy_job_artifacts(
            &source,
            &destination,
            "Mount Rainier",
            &["toposaic.3mf".to_owned(), "manifest.json".to_owned()],
        )
        .await
        .unwrap();
        assert_eq!(files, 3, "the setup counts as a saved file");

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(folder.join(SETUP_FILE_NAME)).unwrap())
                .unwrap();
        // Exactly the shape app/terrain/studio.tsx imports.
        assert_eq!(written["version"], 1);
        let setups = written["setups"].as_array().unwrap();
        assert_eq!(setups.len(), 1);
        assert_eq!(setups[0]["name"], "Mount Rainier", "named, and trimmed");
        assert_eq!(
            setups[0]["spec"]["rows"], 3,
            "the spec is the manifest's, whatever the editor now holds"
        );

        // A place name that is blank, or missing, still yields a usable one.
        fs::write(
            source.join("manifest.json"),
            br#"{"spec":{"place_name":"   ","rows":3}}"#,
        )
        .unwrap();
        let (blank, _) = copy_job_artifacts(
            &source,
            &destination,
            "Mount Rainier",
            &["manifest.json".to_owned()],
        )
        .await
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(blank.join(SETUP_FILE_NAME)).unwrap())
                .unwrap();
        assert_eq!(written["setups"][0]["name"], "TopoSaic setup");

        // A manifest that carries no spec fails the save rather than
        // leaving a folder with a setup missing from it.
        fs::write(source.join("manifest.json"), br#"{"generator":"x"}"#).unwrap();
        let before = fs::read_dir(&destination).unwrap().count();
        assert!(
            copy_job_artifacts(
                &source,
                &destination,
                "Mount Rainier",
                &["manifest.json".to_owned()],
            )
            .await
            .is_err()
        );
        assert_eq!(fs::read_dir(&destination).unwrap().count(), before);

        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn saving_all_files_copies_only_what_it_was_asked_for() {
        let root = std::env::temp_dir().join(format!("toposaic-saveall-{}", Uuid::new_v4()));
        let source = root.join("job");
        let destination = root.join("picked");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("toposaic.3mf"), b"first").unwrap();
        fs::write(
            source.join("manifest.json"),
            br#"{"spec":{"place_name":"Mount Rainier","relief_mm":28.0}}"#,
        )
        .unwrap();
        fs::write(source.join("piece-1-1.stl"), b"stl").unwrap();
        fs::write(source.join("preview.json"), b"preview").unwrap();

        let print_files = ["toposaic.3mf".to_owned(), "manifest.json".to_owned()];
        let (first, files) =
            copy_job_artifacts(&source, &destination, "Mount Rainier", &print_files)
                .await
                .unwrap();
        // The two named files, plus the setup that made them.
        assert_eq!(files, 3);
        assert_eq!(first.file_name().unwrap(), "Mount-Rainier");
        assert_eq!(fs::read(first.join("toposaic.3mf")).unwrap(), b"first");
        // The app's own preview data, and an STL nobody asked for, stay put.
        assert!(!first.join("preview.json").exists());
        assert!(!first.join("piece-1-1.stl").exists());

        // A second save of a job by the same name lands beside the first,
        // with the first left exactly as it was.
        fs::write(source.join("toposaic.3mf"), b"second").unwrap();
        let (again, _) = copy_job_artifacts(&source, &destination, "Mount Rainier", &print_files)
            .await
            .unwrap();
        assert_ne!(again, first);
        assert_eq!(again.file_name().unwrap(), "Mount-Rainier-1");
        assert_eq!(fs::read(first.join("toposaic.3mf")).unwrap(), b"first");
        assert_eq!(fs::read(again.join("toposaic.3mf")).unwrap(), b"second");

        // A folder that is not there is refused rather than created.
        assert!(
            copy_job_artifacts(&source, &root.join("missing"), "Rainier", &print_files)
                .await
                .is_err()
        );

        // A name that tries to leave the job folder, or that is not there,
        // fails before anything is written.
        let before = fs::read_dir(&destination).unwrap().count();
        assert!(
            copy_job_artifacts(
                &source,
                &destination,
                "Rainier",
                &["../secrets.txt".to_owned()],
            )
            .await
            .is_err()
        );
        assert!(
            copy_job_artifacts(&source, &destination, "Rainier", &["gone.3mf".to_owned()])
                .await
                .is_err()
        );
        assert!(
            copy_job_artifacts(&source, &destination, "Rainier", &[])
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_dir(&destination).unwrap().count(),
            before,
            "a refused save leaves no half-made folder behind"
        );

        // A copy that fails PART WAY takes its folder with it, rather than
        // leaving one that holds half a job and looks finished. An
        // unreadable second file gets past the is-it-there check and then
        // fails the read, which is the shape of a full disk or a pulled
        // drive.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let locked = source.join("locked.3mf");
            fs::write(&locked, b"locked").unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
            // Running as root would read it anyway and prove nothing.
            if fs::read(&locked).is_err() {
                let before = fs::read_dir(&destination).unwrap().count();
                assert!(
                    copy_job_artifacts(
                        &source,
                        &destination,
                        "Rainier",
                        &["toposaic.3mf".to_owned(), "locked.3mf".to_owned()],
                    )
                    .await
                    .is_err()
                );
                assert_eq!(
                    fs::read_dir(&destination).unwrap().count(),
                    before,
                    "a copy that stops halfway leaves no folder behind"
                );
            }
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        }

        fs::remove_dir_all(root).unwrap();
    }
}
