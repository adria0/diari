use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("perb=info")).init();
    let data_dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("perb");
    perb::run(
        data_dir,
        |cfg| {
            cfg.notes_dir.clone().unwrap_or_else(|| {
                dirs::document_dir()
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("perb")
            })
        },
        true,
    )
}
