use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use anyhow::Result;
use std::fs;

#[derive(Debug, Clone)]
pub struct PackageInstance {
	pub name: String,
	pub version: String,
	/// dependency name -> range (for now) – we resolve to concrete version via instance map later
	pub dependencies: BTreeMap<String, String>,
	/// hex hash of the stored package content (sha512 hex)
	pub store_hex: String,
}

#[derive(Debug)]
pub struct Linker {
	pub virtual_dir_name: String, // e.g. ".pacm" (analogous to pnpm's .pnpm)
}

impl Linker {
	pub fn new() -> Self { Self { virtual_dir_name: ".pacm".into() } }

	/// Create the virtual store layout and public facade.
	/// instances: map of package name -> instance (currently single version per name)
	/// root_deps: direct dependencies declared in manifest (names)
	pub fn link_project(&self, project_root: &Path, instances: &HashMap<String, PackageInstance>, root_deps: &BTreeMap<String, String>) -> Result<()> {
		let node_modules = project_root.join("node_modules");
		fs::create_dir_all(&node_modules)?;
		let virtual_root = node_modules.join(&self.virtual_dir_name);
		fs::create_dir_all(&virtual_root)?;

		// 1. Create each instance directory with a directory-level symlink/junction into the store
		for inst in instances.values() {
			let inst_dir_name = format!("{}@{}", inst.name, inst.version);
			let inst_dir = virtual_root.join(&inst_dir_name);
			let pkg_parent = inst_dir.join("node_modules");
			let pkg_dir = pkg_parent.join(&inst.name);
			if !pkg_dir.exists() {
				fs::create_dir_all(&pkg_parent)?;
				let store_pkg_dir = crate::store::package_path(&inst.store_hex);
				symlink_dir_with_fallback(&store_pkg_dir, &pkg_dir)?;
			}
			let nested_nm = pkg_dir.join("node_modules");
			fs::create_dir_all(&nested_nm)?; // holds dependency symlinks to other instances
		}

		// 2. Wire dependency symlinks (or junctions). For each package, create entries for deps.
		for inst in instances.values() {
			let inst_dir_name = format!("{}@{}", inst.name, inst.version);
			let pkg_dir = virtual_root.join(&inst_dir_name).join("node_modules").join(&inst.name);
			let nested_nm = pkg_dir.join("node_modules");
			for dep_name in inst.dependencies.keys() {
				if let Some(dep_inst) = instances.get(dep_name) {
					let dep_inst_dir_name = format!("{}@{}", dep_inst.name, dep_inst.version);
					let target_pkg_dir = virtual_root.join(&dep_inst_dir_name).join("node_modules").join(&dep_inst.name);
					let link_path = nested_nm.join(dep_name);
					if link_path.exists() { continue; }
					symlink_dir_with_fallback(&target_pkg_dir, &link_path)?;
				}
			}
		}

		// 3. Public facade: top-level symlinks for root dependencies
		for dep_name in root_deps.keys() {
			if let Some(inst) = instances.get(dep_name) {
				let inst_dir_name = format!("{}@{}", inst.name, inst.version);
				let target_pkg_dir = virtual_root.join(&inst_dir_name).join("node_modules").join(&inst.name);
				let public_path = node_modules.join(dep_name);
				if public_path.exists() { continue; }
				symlink_dir_with_fallback(&target_pkg_dir, &public_path)?;
			}
		}

		Ok(())
	}
}

fn symlink_dir_with_fallback(from: &Path, to: &Path) -> Result<()> {
	if to.exists() { return Ok(()); }
	#[cfg(windows)]
	{
		if let Err(e) = symlink_dir_with_junction_fallback(from, to) {
			if let Some(1314) = e.raw_os_error() { // privilege error
				copy_dir_recursive(from, to)?;
				eprintln!("Warning: lacked privilege to create symlink/junction; copied {:?}", to);
			} else { return Err(e.into()); }
		}
		return Ok(());
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::symlink;
		if let Err(e) = symlink(from, to) { copy_dir_recursive(from, to)?; eprintln!("Warning: symlink fallback copy {:?}: {}", to, e); }
		return Ok(());
	}
	#[cfg(not(any(unix, windows)))]
	{
		copy_dir_recursive(from, to)?; return Ok(());
	}
}

#[cfg(windows)]
fn symlink_dir_with_junction_fallback(from: &Path, to: &Path) -> std::io::Result<()> {
	use std::os::windows::fs::symlink_dir;
	match symlink_dir(from, to) {
		Ok(_) => Ok(()),
		Err(orig_err) => {
			if let Some(1314) = orig_err.raw_os_error() { // need admin, try junction
				use std::process::{Command, Stdio};
				let output = Command::new("cmd")
					.args(["/C", "mklink", "/J", to.to_str().unwrap(), from.to_str().unwrap()])
					.stdout(Stdio::null())
					.stderr(Stdio::null())
					.output()?;
				if output.status.success() { Ok(()) } else { Err(orig_err) }
			} else { Err(orig_err) }
		}
	}
}

fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
	if !to.exists() { fs::create_dir_all(to)?; }
	for entry in fs::read_dir(from)? {
		let entry = entry?; let p = entry.path(); let meta = entry.metadata()?; let dst = to.join(entry.file_name());
		if meta.is_dir() { copy_dir_recursive(&p, &dst)?; } else { std::fs::copy(&p, &dst)?; }
	}
	Ok(())
}

