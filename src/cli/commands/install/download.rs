use crate::fetch::Fetcher;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Debug)]
pub(super) struct DownloadProgress {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) downloaded: u64,
    pub(super) total: Option<u64>,
    pub(super) done: bool,
    pub(super) started_at: Instant,
}

pub(super) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value > 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{}{}", bytes, UNITS[idx])
    } else {
        format!("{:.1}{}", value, UNITS[idx])
    }
}

pub(super) fn perform_download(
    fetcher: &Fetcher,
    name: &str,
    version: &str,
    url: &str,
    downloads: &Arc<Mutex<Vec<DownloadProgress>>>,
) -> Result<Vec<u8>> {
    let entry_index: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let downloads_clone = downloads.clone();
    let package_name = name.to_string();
    let package_version = version.to_string();
    let bytes = fetcher.download_tarball_stream(url, |downloaded, total| {
        if total.unwrap_or(0) > 10 * 1024 * 1024 {
            let mut idx_lock = entry_index.lock().unwrap();
            if idx_lock.is_none() {
                let mut all = downloads_clone.lock().unwrap();
                all.push(DownloadProgress {
                    name: package_name.clone(),
                    version: package_version.clone(),
                    downloaded,
                    total,
                    done: false,
                    started_at: Instant::now(),
                });
                *idx_lock = Some(all.len() - 1);
            } else if let Some(i) = *idx_lock {
                let mut all = downloads_clone.lock().unwrap();
                if let Some(entry) = all.get_mut(i) {
                    entry.downloaded = downloaded;
                    entry.total = total;
                    if total.map(|t| downloaded >= t).unwrap_or(false) {
                        entry.done = true;
                    }
                }
            }
        }
    })?;

    if let Some(idx) = *entry_index.lock().unwrap() {
        let mut all = downloads.lock().unwrap();
        if let Some(entry) = all.get_mut(idx) {
            entry.downloaded = entry.total.unwrap_or(entry.downloaded);
            entry.done = true;
        }
    }
    Ok(bytes)
}
