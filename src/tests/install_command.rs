use super::common::lock_env;
use crate::cache::cache_package_path;
use crate::cli::commands::{
    cmd_scripts_run,
    install::{cmd_install, InstallOptions},
};
use crate::lockfile::Lockfile;
use anyhow::Result;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tempfile::{tempdir, TempDir};

static TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

struct EnvSandbox {
    _env_guard: MutexGuard<'static, ()>,
    temp: TempDir,
    prev_xdg: Option<OsString>,
    prev_local: Option<OsString>,
    prev_appdata: Option<OsString>,
    prev_home: Option<OsString>,
}

impl EnvSandbox {
    fn new() -> Self {
        let env_guard = lock_env();
        let temp = tempdir().expect("create sandbox tempdir");
        let data_home = temp.path().join("data-home");
        fs::create_dir_all(&data_home).expect("create data-home dir");

        let prev_xdg = env::var_os("XDG_DATA_HOME");
        env::set_var("XDG_DATA_HOME", &data_home);

        let prev_local = env::var_os("LOCALAPPDATA");
        env::set_var("LOCALAPPDATA", &data_home);

        let prev_appdata = env::var_os("APPDATA");
        env::set_var("APPDATA", &data_home);

        let prev_home = env::var_os("HOME");
        env::set_var("HOME", temp.path());

        Self { _env_guard: env_guard, temp, prev_xdg, prev_local, prev_appdata, prev_home }
    }

    fn project_root(&self) -> PathBuf {
        self.temp.path().join("project")
    }
}

impl Drop for EnvSandbox {
    fn drop(&mut self) {
        restore_env("XDG_DATA_HOME", &self.prev_xdg);
        restore_env("LOCALAPPDATA", &self.prev_local);
        restore_env("APPDATA", &self.prev_appdata);
        restore_env("HOME", &self.prev_home);
    }
}

fn restore_env(key: &str, previous: &Option<OsString>) {
    if let Some(val) = previous {
        env::set_var(key, val);
    } else {
        env::remove_var(key);
    }
}

struct CwdGuard {
    prev: PathBuf,
}

impl CwdGuard {
    fn change_to(dir: &Path) -> std::io::Result<Self> {
        let prev = env::current_dir()?;
        env::set_current_dir(dir)?;
        Ok(Self { prev })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.prev);
    }
}

fn write_project_manifest(project_root: &Path, manifest: &Value) {
    fs::create_dir_all(project_root).expect("create project dir");
    let manifest_path = project_root.join("package.json");
    let data = serde_json::to_string_pretty(manifest).expect("serialize manifest");
    fs::write(manifest_path, data).expect("write package.json");
}

fn seed_cached_package(name: &str, version: &str, manifest: Value, files: &[(&str, &str)]) {
    let dir = cache_package_path(name, version);
    fs::create_dir_all(&dir).expect("create cached package dir");
    let manifest_path = dir.join("package.json");
    fs::write(&manifest_path, manifest.to_string()).expect("write cached manifest");
    if let Some(scripts_val) = manifest.get("scripts") {
        let sidecar_path = dir.join(".registry-scripts.json");
        if let Ok(txt) = serde_json::to_string_pretty(scripts_val) {
            let _ = fs::write(&sidecar_path, txt);
        }
    }
    for (rel, contents) in files {
        let file_path = dir.join(rel);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(file_path, contents).expect("write cached file");
    }
}

fn lockfile_path(project_root: &Path) -> PathBuf {
    project_root.join("pacm.lockb")
}

fn install_options_copy() -> InstallOptions {
    InstallOptions { copy: true, no_progress: true, ..InstallOptions::default() }
}

fn host_node_platform() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        return "win32";
    }
    #[cfg(target_os = "macos")]
    {
        return "darwin";
    }
    #[cfg(target_os = "linux")]
    {
        return "linux";
    }
    #[cfg(target_os = "freebsd")]
    {
        return "freebsd";
    }
    #[cfg(target_os = "openbsd")]
    {
        return "openbsd";
    }
    #[cfg(target_os = "netbsd")]
    {
        return "netbsd";
    }
    #[cfg(target_os = "aix")]
    {
        return "aix";
    }
    #[cfg(target_os = "solaris")]
    {
        return "sunos";
    }
    #[allow(unreachable_code)]
    "unknown"
}

#[test]
fn scripts_run_executes_registry_scripts() -> Result<()> {
    let _guard = match TEST_MUTEX.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let sandbox = EnvSandbox::new();
    let project_root = sandbox.project_root();

    #[cfg(windows)]
    let scripts = json!({
        "preinstall": "cmd /C echo pre > pre.txt",
        "install": "cmd /C echo inst > inst.txt",
        "postinstall": "cmd /C echo post > post.txt",
    });
    #[cfg(not(windows))]
    let scripts = json!({
        "preinstall": "sh -c 'echo pre > pre.txt'",
        "install": "sh -c 'echo inst > inst.txt'",
        "postinstall": "sh -c 'echo post > post.txt'",
    });

    write_project_manifest(
        &project_root,
        &json!({
            "name": "script-app",
            "version": "0.1.0",
            "dependencies": { "scripty": "1.0.0" }
        }),
    );

    seed_cached_package(
        "scripty",
        "1.0.0",
        json!({ "name": "scripty", "version": "1.0.0", "scripts": scripts }),
        &[("index.js", "module.exports = 'scripty';\n")],
    );

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(Vec::new(), install_options_copy())?;

    cmd_scripts_run(vec!["scripty".to_string()], false, false, true, false)?;

    let sdir = project_root.join("node_modules").join("scripty");
    assert!(sdir.join("pre.txt").exists());
    assert!(sdir.join("inst.txt").exists());
    assert!(sdir.join("post.txt").exists());

    Ok(())
}

#[test]
fn installs_cached_packages_and_updates_lock() -> Result<()> {
    let _guard = match TEST_MUTEX.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let sandbox = EnvSandbox::new();
    let project_root = sandbox.project_root();
    write_project_manifest(
        &project_root,
        &json!({
            "name": "demo-app",
            "version": "0.1.0",
            "dependencies": {
                "alpha": "1.0.0",
                "beta": "2.0.0"
            },
            "optionalDependencies": {
                "optional-root": "1.0.0"
            }
        }),
    );

    let host_os = host_node_platform();
    let block_os = format!("!{host_os}");

    seed_cached_package(
        "alpha",
        "1.0.0",
        json!({
            "name": "alpha",
            "version": "1.0.0",
            "dependencies": { "gamma": "1.0.0" },
            "optionalDependencies": { "alpha-optional": "1.0.0" },
            "bin": { "alpha": "bin.js" }
        }),
        &[("bin.js", "#!/usr/bin/env node\nconsole.log('alpha');\n")],
    );

    seed_cached_package(
        "beta",
        "2.0.0",
        json!({
            "name": "beta",
            "version": "2.0.0",
            "peerDependencies": { "gamma": "^1.0.0" }
        }),
        &[("index.js", "module.exports = 'beta';\n")],
    );

    seed_cached_package(
        "gamma",
        "1.0.0",
        json!({
            "name": "gamma",
            "version": "1.0.0",
            "bin": { "gamma-cli": "cli.js" }
        }),
        &[("cli.js", "#!/usr/bin/env node\nconsole.log('gamma');\n")],
    );

    seed_cached_package(
        "alpha-optional",
        "1.0.0",
        json!({
            "name": "alpha-optional",
            "version": "1.0.0",
            "os": [block_os.clone()]
        }),
        &[("index.js", "module.exports = 'optional';\n")],
    );

    seed_cached_package(
        "optional-root",
        "1.0.0",
        json!({
            "name": "optional-root",
            "version": "1.0.0",
            "os": [block_os.clone()]
        }),
        &[("root.js", "module.exports = 'optional-root';\n")],
    );

    let _cwd = CwdGuard::change_to(&project_root)?;
    let options = install_options_copy();
    cmd_install(Vec::new(), options)?;

    let bin_dir = project_root.join("node_modules").join(".bin");
    #[cfg(windows)]
    let gamma_bin = bin_dir.join("gamma-cli.exe");
    #[cfg(not(windows))]
    let gamma_bin = bin_dir.join("gamma-cli");
    let bin_listing: Vec<String> = fs::read_dir(&bin_dir)
        .map(|iter| {
            iter.filter_map(|entry| entry.ok().and_then(|e| e.file_name().into_string().ok()))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        gamma_bin.exists(),
        "gamma bin shim missing; bin_dir_exists={} entries={:?}",
        bin_dir.exists(),
        bin_listing
    );

    let alpha_dir = project_root.join("node_modules").join("alpha");
    assert!(alpha_dir.join("bin.js").exists());
    let gamma_dir = project_root.join("node_modules").join("gamma");
    assert!(gamma_dir.join("cli.js").exists());

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    assert!(lock.packages.get("node_modules/alpha").is_some());
    assert!(lock.packages.get("node_modules/gamma").is_some());
    if let Some(optional_entry) = lock.packages.get("node_modules/optional-root") {
        assert_eq!(optional_entry.version.as_deref(), Some("1.0.0"));
        assert_eq!(optional_entry.os, vec![block_os.clone()]);
        assert!(optional_entry.store_key.is_none());
    } else {
        panic!("optional-root entry missing from lockfile");
    }

    Ok(())
}

#[test]
fn reinstall_prunes_removed_packages() -> Result<()> {
    let _guard = match TEST_MUTEX.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let sandbox = EnvSandbox::new();
    let project_root = sandbox.project_root();
    write_project_manifest(
        &project_root,
        &json!({
            "name": "demo-app",
            "version": "0.1.0",
            "dependencies": { "delta": "1.0.0", "epsilon": "1.0.0" }
        }),
    );

    seed_cached_package(
        "delta",
        "1.0.0",
        json!({ "name": "delta", "version": "1.0.0" }),
        &[("index.js", "module.exports = 'delta';\n")],
    );

    seed_cached_package(
        "epsilon",
        "1.0.0",
        json!({ "name": "epsilon", "version": "1.0.0" }),
        &[("index.js", "module.exports = 'epsilon';\n")],
    );

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(Vec::new(), install_options_copy())?;

    write_project_manifest(
        &project_root,
        &json!({
            "name": "demo-app",
            "version": "0.1.0",
            "dependencies": { "delta": "1.0.0" }
        }),
    );

    cmd_install(Vec::new(), install_options_copy())?;

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    assert!(lock.packages.get("node_modules/delta").is_some());
    assert!(lock.packages.get("node_modules/epsilon").is_none());

    let epsilon_dir = project_root.join("node_modules").join("epsilon");
    assert!(!epsilon_dir.exists(), "epsilon directory should be pruned");
    Ok(())
}

#[test]
fn install_from_specs_updates_manifest() -> Result<()> {
    let _guard = match TEST_MUTEX.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let sandbox = EnvSandbox::new();
    let project_root = sandbox.project_root();
    write_project_manifest(
        &project_root,
        &json!({
            "name": "spec-app",
            "version": "0.1.0",
            "dependencies": {}
        }),
    );

    seed_cached_package(
        "zeta",
        "1.0.0",
        json!({ "name": "zeta", "version": "1.0.0" }),
        &[("index.js", "module.exports = 'zeta';\n")],
    );

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(vec!["zeta@1.0.0".to_string()], install_options_copy())?;

    let manifest_text = fs::read_to_string(project_root.join("package.json"))?;
    let manifest_json: Value = serde_json::from_str(&manifest_text)?;
    let deps = manifest_json
        .get("dependencies")
        .and_then(|v| v.as_object())
        .expect("dependencies present");
    assert_eq!(deps.get("zeta").and_then(|v| v.as_str()), Some("1.0.0"));

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    assert!(lock.packages.get("node_modules/zeta").is_some());
    Ok(())
}
