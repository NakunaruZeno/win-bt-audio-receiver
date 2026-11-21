use std::fs;
use std::io;
use std::path::PathBuf;

pub const CONFIG_FILE_NAME: &str = "device_id.cfg";

#[derive(Clone, Debug, Default)]
pub struct SavedConfig {
    pub bt_id: Option<String>,
    pub bt_name: Option<String>,
    pub adb_host: Option<String>,
}

pub fn config_file_path() -> PathBuf {
    match std::env::current_exe() {
        Ok(exe) => match exe.parent() {
            Some(dir) => dir.join(CONFIG_FILE_NAME),
            None => PathBuf::from(CONFIG_FILE_NAME),
        },
        Err(_) => PathBuf::from(CONFIG_FILE_NAME),
    }
}

pub fn load_config() -> SavedConfig {
    let path = config_file_path();
    let Ok(content) = fs::read_to_string(path) else {
        return SavedConfig::default();
    };
    let mut lines = content.lines();
    let bt_id = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let bt_name = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let adb_host = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    SavedConfig {
        bt_id,
        bt_name,
        adb_host,
    }
}

pub fn load_saved_bt_device() -> Option<(String, String)> {
    let cfg = load_config();
    let id = cfg.bt_id?;
    let name = cfg
        .bt_name
        .unwrap_or_else(|| "Saved Bluetooth Device".to_owned());
    Some((id, name))
}

pub fn save_bt_device(id: &str, name: &str) -> io::Result<()> {
    let mut cfg = load_config();
    cfg.bt_id = Some(id.trim().to_owned());
    cfg.bt_name = Some(name.replace(['\r', '\n'], " ").trim().to_owned());
    write_config(&cfg)?;
    println!("[Config] Saved Bluetooth device to {}", config_file_path().display());
    Ok(())
}

pub fn save_adb_host(host: &str) -> io::Result<()> {
    let mut cfg = load_config();
    cfg.adb_host = Some(host.trim().to_owned());
    write_config(&cfg)
}

fn write_config(cfg: &SavedConfig) -> io::Result<()> {
    let path = config_file_path();
    let l1 = cfg.bt_id.as_deref().unwrap_or("");
    let l2 = cfg.bt_name.as_deref().unwrap_or("");
    let l3 = cfg.adb_host.as_deref().unwrap_or("");
    fs::write(path, format!("{l1}\n{l2}\n{l3}\n"))
}
