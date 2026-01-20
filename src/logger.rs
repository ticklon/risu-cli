use crate::config;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

static LOG_FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

pub fn init() {
    let mut log_dir = config::get_config_dir();
    log_dir.push("logs");
    fs::create_dir_all(&log_dir).ok();

    let mut log_path = log_dir.clone();
    log_path.push("risu.log");

    // Rotate log file: move old log to risu.log.old
    if log_path.exists() {
        let mut old_path = log_dir;
        old_path.push("risu.log.old");
        // Try to remove old log first to ensure rename succeeds (simple rotation)
        if old_path.exists() {
            let _ = fs::remove_file(&old_path);
        }
        let _ = fs::rename(&log_path, old_path);
    }

    let mut options = OpenOptions::new();
    options.create(true).append(true); // Append to new file

    #[cfg(unix)]
    {
        options.mode(0o600);
    }

    let file = options.open(log_path).expect("Failed to open log file");

    let _ = LOG_FILE.set(Mutex::new(file));
}

pub fn log(msg: &str) {
    if let Some(mutex) = LOG_FILE.get() {
        if let Ok(mut file) = mutex.lock() {
            let _ = writeln!(file, "[{}] {}", chrono::Local::now(), msg);
        }
    }
}
