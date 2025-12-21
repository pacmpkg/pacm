use crate::cache::cache_package_path;
use crate::cli::commands::install::{cmd_install, InstallOptions};
use crate::lockfile::Lockfile;
use crate::tests::common::DataHomeGuard;
use anyhow::Result;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn install_options_copy() -> InstallOptions {
    InstallOptions { copy: true, no_progress: true, ..InstallOptions::default() }
}

struct CwdGuard {
    prev: PathBuf,
}

impl CwdGuard {
    fn change_to(dir: &Path) -> std::io::Result<Self> {
        let prev = std::env::current_dir()?;
        std::env::set_current_dir(dir)?;
        Ok(Self { prev })
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

fn write_manifest(path: &Path, manifest: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create manifest parent");
    }
    let data = serde_json::to_string_pretty(manifest).expect("serialize manifest");
    fs::write(path, data).expect("write manifest");
}

fn seed_cached_package(name: &str, version: &str, manifest: Value, files: &[(&str, &str)]) {
    let dir = cache_package_path(name, version);
    fs::create_dir_all(&dir).expect("create cached package dir");
    let manifest_path = dir.join("package.json");
    fs::write(&manifest_path, manifest.to_string()).expect("write cached manifest");
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

#[test]
fn installs_workspace_package_and_links_it() -> Result<()> {
    let _guard = DataHomeGuard::new();
    let temp = tempdir()?;
    let project_root = temp.path().join("project");

    write_manifest(
        &project_root.join("package.json"),
        &json!({
            "name": "root-app",
            "version": "0.1.0",
            "workspaces": ["packages/*"],
            "dependencies": { "pkg-a": "workspace:*" }
        }),
    );

    let ws_dir = project_root.join("packages").join("pkg-a");
    write_manifest(
        &ws_dir.join("package.json"),
        &json!({
            "name": "pkg-a",
            "version": "1.2.3"
        }),
    );
    fs::write(ws_dir.join("index.js"), "module.exports = 'pkg-a';\n")?;

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(Vec::new(), install_options_copy())?;

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    let entry =
        lock.packages.get("node_modules/pkg-a").expect("workspace package recorded in lockfile");
    assert_eq!(entry.version.as_deref(), Some("1.2.3"));
    assert!(entry.resolved.as_deref().unwrap_or_default().starts_with("workspace:"));

    let installed_pkg = project_root.join("node_modules").join("pkg-a");
    assert!(installed_pkg.join("package.json").exists());
    Ok(())
}

#[test]
fn installs_workspace_dev_dependencies() -> Result<()> {
    let _guard = DataHomeGuard::new();
    let temp = tempdir()?;
    let project_root = temp.path().join("project");

    seed_cached_package(
        "dev-helper",
        "1.0.0",
        json!({ "name": "dev-helper", "version": "1.0.0" }),
        &[("index.js", "module.exports = 'dev-helper';\n")],
    );

    write_manifest(
        &project_root.join("package.json"),
        &json!({
            "name": "root-app",
            "version": "0.1.0",
            "workspaces": ["packages/*"],
            "dependencies": { "pkg-c": "workspace:*" }
        }),
    );

    let ws_dir = project_root.join("packages").join("pkg-c");
    write_manifest(
        &ws_dir.join("package.json"),
        &json!({
            "name": "pkg-c",
            "version": "1.0.0",
            "devDependencies": { "dev-helper": "1.0.0" }
        }),
    );
    fs::write(ws_dir.join("index.js"), "module.exports = 'pkg-c';\n")?;

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(Vec::new(), install_options_copy())?;

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    assert!(lock.packages.get("node_modules/dev-helper").is_some());

    let dev_pkg = project_root.join("node_modules").join("dev-helper");
    assert!(dev_pkg.exists());
    Ok(())
}

#[test]
fn resolves_workspace_dependency_by_version_range() -> Result<()> {
    let _guard = DataHomeGuard::new();
    let temp = tempdir()?;
    let project_root = temp.path().join("project");

    write_manifest(
        &project_root.join("package.json"),
        &json!({
            "name": "root-app",
            "version": "0.1.0",
            "workspaces": ["packages/*"],
            "dependencies": { "pkg-a": "workspace:*" }
        }),
    );

    let pkg_b = project_root.join("packages").join("pkg-b");
    write_manifest(&pkg_b.join("package.json"), &json!({ "name": "pkg-b", "version": "1.0.0" }));

    let pkg_a = project_root.join("packages").join("pkg-a");
    write_manifest(
        &pkg_a.join("package.json"),
        &json!({
            "name": "pkg-a",
            "version": "1.0.0",
            "dependencies": { "pkg-b": "^1.0.0" }
        }),
    );

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(Vec::new(), install_options_copy())?;

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    let b_entry = lock.packages.get("node_modules/pkg-b").expect("pkg-b in lock");
    assert_eq!(b_entry.version.as_deref(), Some("1.0.0"));
    assert!(b_entry.resolved.as_deref().unwrap_or_default().starts_with("workspace:"));

    let nm = project_root.join("node_modules");
    assert!(nm.join("pkg-b").exists());
    assert!(nm.join("pkg-a").exists());
    Ok(())
}

#[test]
fn installs_scoped_workspace_chain() -> Result<()> {
    let _guard = DataHomeGuard::new();
    let temp = tempdir()?;
    let project_root = temp.path().join("lumix");

    write_manifest(
        &project_root.join("package.json"),
        &json!({
            "name": "lumix-platform",
            "version": "0.1.0",
            "workspaces": ["packages/*"],
            "dependencies": { "@lumix/api": "workspace:^1.0.0" }
        }),
    );

    let logger_dir = project_root.join("packages").join("logger");
    write_manifest(
        &logger_dir.join("package.json"),
        &json!({ "name": "@lumix/logger", "version": "1.0.0", "main": "index.js" }),
    );
    fs::write(
        logger_dir.join("index.js"),
        "module.exports.log = (m) => console.log('[lumix]', m);\n",
    )?;

    let api_dir = project_root.join("packages").join("api");
    write_manifest(
        &api_dir.join("package.json"),
        &json!({
            "name": "@lumix/api",
            "version": "1.0.0",
            "main": "index.js",
            "dependencies": { "@lumix/logger": "workspace:^1.0.0" }
        }),
    );
    fs::write(
        api_dir.join("index.js"),
        "const { log } = require('@lumix/logger');\nmodule.exports = { ping: () => log('api up') };\n",
    )?;

    let _cwd = CwdGuard::change_to(&project_root)?;
    cmd_install(Vec::new(), install_options_copy())?;

    let lock = Lockfile::load_or_default(lockfile_path(&project_root))?;
    let api_entry = lock.packages.get("node_modules/@lumix/api").expect("api lock entry");
    assert_eq!(api_entry.version.as_deref(), Some("1.0.0"));
    assert!(api_entry.resolved.as_deref().unwrap_or_default().starts_with("workspace:"));
    assert_eq!(
        api_entry.dependencies.get("@lumix/logger").map(|s| s.as_str()),
        Some("workspace:^1.0.0")
    );

    let logger_entry = lock.packages.get("node_modules/@lumix/logger").expect("logger lock entry");
    assert_eq!(logger_entry.version.as_deref(), Some("1.0.0"));
    assert!(logger_entry.resolved.as_deref().unwrap_or_default().starts_with("workspace:"));

    let nm = project_root.join("node_modules");
    assert!(nm.join("@lumix").join("api").exists());
    assert!(nm.join("@lumix").join("logger").exists());
    Ok(())
}
