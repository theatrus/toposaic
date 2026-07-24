use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

static ENGINE_STARTED: OnceLock<()> = OnceLock::new();

fn source_artifact_path(
    data_dir: &Path,
    job_id: &str,
    artifact_name: &str,
) -> Result<PathBuf, String> {
    let job_id = Uuid::parse_str(job_id)
        .map_err(|_| "The job ID is not valid.".to_owned())?
        .hyphenated()
        .to_string();
    let output_dir = data_dir.join("jobs").join(job_id);
    terrain_core::artifact_path(&output_dir, artifact_name)
        .ok_or_else(|| "The requested print file does not exist.".to_owned())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![save_artifact])
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
                        terrain_api::run_with(data_dir, "127.0.0.1:38787".into()).await
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
}
