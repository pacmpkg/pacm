use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use anyhow::Result;
use std::fs;
use once_cell::sync::Lazy;
// record_warning only invoked for unexpected junction errors (inline path to avoid unused warnings)

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
		// IMPORTANT: Previously we symlinked the package directory directly to the global store.
		// That causes Node to resolve module files from the store path, bypassing the virtual
		// layout where dependency symlinks live, so requires like `require("body-parser")`
		// from inside express failed. Instead we now MATERIALIZE (copy or hard-link future) the
		// package contents into the virtual store instance directory. Dependencies continue to
		// be wired as symlinks at the sibling level (instance/node_modules/<dep>).
		for inst in instances.values() {
			let inst_dir_name = format!("{}@{}", inst.name, inst.version);
			let inst_dir = virtual_root.join(&inst_dir_name);
			let pkg_parent = inst_dir.join("node_modules");
			let mut pkg_dir = pkg_parent.clone();
			for part in inst.name.split('/') { pkg_dir = pkg_dir.join(part); }
			if let Some(parent) = pkg_dir.parent() { fs::create_dir_all(parent)?; }
			let store_pkg_dir = crate::store::package_path(&inst.store_hex);
			materialize_package_dir(&store_pkg_dir, &pkg_dir)?;
		}

		// 2. Wire dependency symlinks (or junctions). For each package, create entries for deps.
		// Place them at the instance-level node_modules so Node can resolve from the parent dir of the package.
		for inst in instances.values() {
			let inst_dir_name = format!("{}@{}", inst.name, inst.version);
			let inst_nm = virtual_root.join(&inst_dir_name).join("node_modules");
			for dep_name in inst.dependencies.keys() {
				if let Some(dep_inst) = instances.get(dep_name) {
					let dep_inst_dir_name = format!("{}@{}", dep_inst.name, dep_inst.version);
					let mut target_pkg_dir = virtual_root.join(&dep_inst_dir_name).join("node_modules");
					for part in dep_inst.name.split('/') { target_pkg_dir = target_pkg_dir.join(part); }
					// build link path inside the instance's node_modules, respecting scope segments
					let mut link_path = inst_nm.clone();
					for part in dep_name.split('/') { link_path = link_path.join(part); }
					if link_path.exists() { continue; }
					if let Some(p) = link_path.parent() { fs::create_dir_all(p)?; }
					symlink_dir_with_fallback(&target_pkg_dir, &link_path)?;
				}
			}
		}

		// 3. Public facade: top-level symlinks for root dependencies
		for dep_name in root_deps.keys() {
			if let Some(inst) = instances.get(dep_name) {
				let inst_dir_name = format!("{}@{}", inst.name, inst.version);
				let mut target_pkg_dir = virtual_root.join(&inst_dir_name).join("node_modules");
				for part in inst.name.split('/') { target_pkg_dir = target_pkg_dir.join(part); }
				let mut public_path = node_modules.clone();
				for part in dep_name.split('/') { public_path = public_path.join(part); }
				if public_path.exists() { continue; }
				if let Some(p) = public_path.parent() { fs::create_dir_all(p)?; }
				symlink_dir_with_fallback(&target_pkg_dir, &public_path)?;
			}
		}

		// 4. Global virtual node_modules inside .pacm for peer dependency fallback resolution
		// Layout mirrors pnpm: node_modules/.pacm/node_modules/<name> -> ../<name>@<ver>/node_modules/<name>
		let global_virtual_nm = virtual_root.join("node_modules");
		fs::create_dir_all(&global_virtual_nm)?;
		for inst in instances.values() {
			let inst_dir_name = format!("{}@{}", inst.name, inst.version);
			let mut target_pkg_dir = virtual_root.join(&inst_dir_name).join("node_modules");
			for part in inst.name.split('/') { target_pkg_dir = target_pkg_dir.join(part); }
			let mut link_path = global_virtual_nm.clone();
			for part in inst.name.split('/') { link_path = link_path.join(part); }
			if link_path.exists() { continue; }
			if let Some(p) = link_path.parent() { fs::create_dir_all(p)?; }
			symlink_dir_with_fallback(&target_pkg_dir, &link_path)?;
		}

		Ok(())
	}
}

fn symlink_dir_with_fallback(from: &Path, to: &Path) -> Result<()> {
	// If destination already exists (including broken symlink/junction), treat as ok.
	// Use symlink_metadata to detect presence even when the symlink target is missing.
	if to.exists() || std::fs::symlink_metadata(to).is_ok() { return Ok(()); }
	#[cfg(windows)]
	{
		static CAN_SYMLINK: Lazy<bool> = Lazy::new(|| {
			use std::os::windows::fs::symlink_dir;
			let tmp = std::env::temp_dir();
			let test_src = tmp.join("pacm_symlink_test_src");
			let test_dst = tmp.join("pacm_symlink_test_dst");
			let _ = std::fs::create_dir_all(&test_src);
			let res = symlink_dir(&test_src, &test_dst);
			let ok = res.is_ok();
			let _ = std::fs::remove_dir_all(&test_dst);
			let _ = std::fs::remove_dir_all(&test_src);
			ok
		});
		if !*CAN_SYMLINK {
			// Directly copy without attempting symlink/junction to avoid repeated failures/noise.
			copy_dir_recursive(from, to)?;
			return Ok(());
		}
		if let Err(e) = symlink_dir_with_junction_fallback(from, to) {
			if let Some(code) = e.raw_os_error() {
				// 1314: privilege required (expected on non-admin); silently copy without warning.
				// 5: access denied (treat similarly if copying succeeds).
				if code == 1314 || code == 5 {
					copy_dir_recursive(from, to)?;
					return Ok(());
				}
			}
			// Other errors may be due to races where the destination was created concurrently.
			if to.exists() || std::fs::symlink_metadata(to).is_ok() { return Ok(()); }
			// Unexpected error: surface as warning then propagate (Windows only)
			#[cfg(windows)] { crate::cli::record_warning(format!("link fallback unexpected error for {:?}: {}", to, e)); }
			return Err(e.into());
		}
		return Ok(());
	}
	#[cfg(unix)]
	{
		use std::os::unix::fs::symlink;
		if let Err(_e) = symlink(from, to) { copy_dir_recursive(from, to)?; }
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
				if output.status.success() { Ok(()) } else {
					if to.exists() || std::fs::symlink_metadata(to).is_ok() { Ok(()) } else { Err(orig_err) }
				}
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

// Materialize a package directory for a virtual instance. If the destination already exists we keep it.
// For now we copy; future optimization: attempt to hard-link individual files (on NTFS use fs::hard_link) to save space.
fn materialize_package_dir(from: &Path, to: &Path) -> Result<()> {
	if to.exists() { return Ok(()); }
	copy_dir_recursive(from, to)
}

