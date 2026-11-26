use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("diari=info")).init();
    let app_data = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let data_dir = app_data.join("diari");
    migrate_dir(&app_data.join("perb"), &data_dir);
    diari::run(
        data_dir,
        |cfg| {
            cfg.notes_dir.clone().unwrap_or_else(|| {
                let docs = dirs::document_dir()
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("."));
                let notes = docs.join("diari");
                migrate_dir(&docs.join("perb"), &notes);
                notes
            })
        },
        true,
    )
}

/// The app was called "perb": move its folder to the new name once, unless
/// the new one already exists.
fn migrate_dir(old: &Path, new: &Path) {
    if old.is_dir() && !new.exists() {
        match std::fs::rename(old, new) {
            Ok(()) => log::info!("moved {} to {}", old.display(), new.display()),
            Err(e) => log::warn!("could not move {} to {}: {e}", old.display(), new.display()),
        }
    }
}
