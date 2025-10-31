use std::env;
use std::ffi::OsString;

/// Guards environment variables so pacm data paths resolve inside a temporary sandbox.
pub struct DataHomeGuard {
    _temp: tempfile::TempDir,
    prev_xdg: Option<OsString>,
    prev_local: Option<OsString>,
    prev_appdata: Option<OsString>,
    prev_home: Option<OsString>,
}

impl DataHomeGuard {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("create test tempdir");
        let data_home = temp.path().join("data-home");
        std::fs::create_dir_all(&data_home).expect("create data-home dir");

        let prev_xdg = env::var_os("XDG_DATA_HOME");
        env::set_var("XDG_DATA_HOME", data_home.as_os_str());

        let prev_local = env::var_os("LOCALAPPDATA");
        env::set_var("LOCALAPPDATA", data_home.as_os_str());

        let prev_appdata = env::var_os("APPDATA");
        env::set_var("APPDATA", data_home.as_os_str());

        let prev_home = env::var_os("HOME");
        env::set_var("HOME", temp.path());

        Self { _temp: temp, prev_xdg, prev_local, prev_appdata, prev_home }
    }
}

impl Drop for DataHomeGuard {
    fn drop(&mut self) {
        restore_env("XDG_DATA_HOME", &self.prev_xdg);
        restore_env("LOCALAPPDATA", &self.prev_local);
        restore_env("APPDATA", &self.prev_appdata);
        restore_env("HOME", &self.prev_home);
        // tempdir drops here and cleans up the sandbox on disk.
    }
}

fn restore_env(key: &str, previous: &Option<OsString>) {
    match previous {
        Some(val) => env::set_var(key, val),
        None => env::remove_var(key),
    }
}
