use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

static ENGINE_STARTED: OnceLock<()> = OnceLock::new();

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
/// Anything that is not a letter, a digit, or a dash collapses to a single
/// dash, so a name the user typed cannot walk out of the folder they chose.
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
    let mut slug = slug.to_owned();
    // Long place names exist; keep the folder name workable on every OS.
    slug.truncate(48);
    slug.trim_end_matches('-').to_owned()
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

/// Copies every artifact of a finished job into a new folder under
/// `destination`. Returns the folder and how many files landed in it.
async fn copy_job_artifacts(
    source_dir: &Path,
    destination: &Path,
    folder_name: &str,
) -> Result<(PathBuf, usize), String> {
    if !destination.is_dir() {
        return Err("The selected folder does not exist.".to_owned());
    }
    let target = free_directory(destination, &folder_slug(folder_name))?;
    tokio::fs::create_dir(&target)
        .await
        .map_err(|error| format!("Could not create {}: {error}", target.display()))?;

    let mut entries = tokio::fs::read_dir(source_dir)
        .await
        .map_err(|error| format!("Could not read the job folder: {error}"))?;
    let mut copied = 0;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| format!("Could not read the job folder: {error}"))?
    {
        if !entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        let name = entry.file_name();
        tokio::fs::copy(entry.path(), target.join(&name))
            .await
            .map_err(|error| format!("Could not save {}: {error}", name.to_string_lossy()))?;
        copied += 1;
    }
    Ok((target, copied))
}

#[tauri::command]
async fn save_artifact(
    app: tauri::AppHandle,
    job_id: String,
    artifact_name: String,
) -> Result<Option<u64>, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not find the TopoSaic data folder: {error}"))?;
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

/// Saves every file of a job in one go: one folder picker, one copy, rather
/// than a save dialog per file — a 10x10 puzzle is over a hundred of them.
#[tauri::command]
async fn save_all_artifacts(
    app: tauri::AppHandle,
    job_id: String,
    folder_name: String,
) -> Result<Option<SavedFolder>, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not find the TopoSaic data folder: {error}"))?;
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

    let (directory, files) = copy_job_artifacts(&source_dir, &destination, &folder_name).await?;
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
        assert!(folder_slug(&"x".repeat(200)).len() <= 48);
    }

    #[tokio::test]
    async fn saving_all_files_never_writes_over_an_earlier_save() {
        let root = std::env::temp_dir().join(format!("toposaic-saveall-{}", Uuid::new_v4()));
        let source = root.join("job");
        let destination = root.join("picked");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("toposaic.3mf"), b"first").unwrap();
        fs::write(source.join("piece-1-1.stl"), b"stl").unwrap();
        fs::create_dir(source.join("scratch")).unwrap();

        let (first, files) = copy_job_artifacts(&source, &destination, "Mount Rainier")
            .await
            .unwrap();
        assert_eq!(files, 2, "the folder inside the job is not an artifact");
        assert_eq!(first.file_name().unwrap(), "Mount-Rainier");
        assert_eq!(fs::read(first.join("toposaic.3mf")).unwrap(), b"first");

        // A second save of a job by the same name lands beside the first,
        // with the first left exactly as it was.
        fs::write(source.join("toposaic.3mf"), b"second").unwrap();
        let (again, _) = copy_job_artifacts(&source, &destination, "Mount Rainier")
            .await
            .unwrap();
        assert_ne!(again, first);
        assert_eq!(again.file_name().unwrap(), "Mount-Rainier-1");
        assert_eq!(fs::read(first.join("toposaic.3mf")).unwrap(), b"first");
        assert_eq!(fs::read(again.join("toposaic.3mf")).unwrap(), b"second");

        // A folder that is not there is refused rather than created.
        assert!(
            copy_job_artifacts(&source, &root.join("missing"), "Mount Rainier")
                .await
                .is_err()
        );

        fs::remove_dir_all(root).unwrap();
    }
}
