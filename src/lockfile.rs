use crate::error::Result;
use crate::manifest::Manifest;
use anyhow::{anyhow, bail, ensure, Context};
use bincode::config::{legacy, standard};
use bincode::serde::decode_from_slice;
use bincode1::Options;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PeerMeta {
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PackageEntry {
    pub version: Option<String>,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub resolved: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependenciesMeta", skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default, rename = "cpu")]
    pub cpu_arch: Vec<String>,
    #[serde(default, rename = "storeKey")]
    pub store_key: Option<String>,
    #[serde(default, rename = "contentHash")]
    pub content_hash: Option<String>,
    #[serde(default, rename = "linkMode")]
    pub link_mode: Option<String>,
    #[serde(default, rename = "storePath")]
    pub store_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub format: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, PackageEntry>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self { format: 1, packages: BTreeMap::new() }
    }
}

impl Lockfile {
    pub fn load_or_default(path: PathBuf) -> Result<Self> {
        if path.exists() {
            load(&path)
        } else {
            Ok(Self::default())
        }
    }

    pub fn sync_from_manifest(&mut self, manifest: &Manifest) {
        let root = self.packages.entry("".into()).or_insert(PackageEntry {
            version: None,
            integrity: None,
            resolved: None,
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            optional_dependencies: BTreeMap::new(),
            peer_dependencies: BTreeMap::new(),
            peer_dependencies_meta: BTreeMap::new(),
            os: Vec::new(),
            cpu_arch: Vec::new(),
            store_key: None,
            content_hash: None,
            link_mode: None,
            store_path: None,
        });
        root.version = Some(manifest.version.clone());
        // Persist each root section separately
        root.dependencies = manifest.dependencies.clone();
        root.dev_dependencies = manifest.dev_dependencies.clone();
        root.optional_dependencies = manifest.optional_dependencies.clone();
        root.peer_dependencies = manifest.peer_dependencies.clone();
        root.peer_dependencies_meta = BTreeMap::new();

        // Collect declared root installable packages (exclude peers) into a vector to avoid borrow conflicts
        let declared: Vec<String> = {
            let r = self.packages.get("").expect("root exists");
            r.dependencies
                .keys()
                .chain(r.dev_dependencies.keys())
                .chain(r.optional_dependencies.keys())
                .cloned()
                .collect()
        };
        // Ensure an entry exists for every declared package (dependencies, dev, optional)
        for name in declared {
            let key = format!("node_modules/{name}");
            self.packages.entry(key).or_insert(PackageEntry {
                version: None,
                integrity: None,
                resolved: None,
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
                optional_dependencies: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
                peer_dependencies_meta: BTreeMap::new(),
                os: Vec::new(),
                cpu_arch: Vec::new(),
                store_key: None,
                content_hash: None,
                link_mode: None,
                store_path: None,
            });
        }
    }
}

const MAX_LOCKFILE_SIZE: usize = 16 * 1024 * 1024;
pub const LOCKFILE_MAGIC: &[u8; 8] = b"PACMLOCK";
const CURRENT_WIRE_VERSION: u16 = 3;

fn write_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_len(buf: &mut Vec<u8>, len: usize, what: &str) -> Result<()> {
    let value = u32::try_from(len).with_context(|| format!("{what} too large"))?;
    write_u32(buf, value);
    Ok(())
}

fn write_string(buf: &mut Vec<u8>, value: &str, what: &str) -> Result<()> {
    write_len(buf, value.len(), what)?;
    buf.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_option_string(buf: &mut Vec<u8>, value: &Option<String>) -> Result<()> {
    match value {
        Some(s) => {
            buf.push(1);
            write_string(buf, s, "string")?
        }
        None => buf.push(0),
    }
    Ok(())
}

fn write_string_map(buf: &mut Vec<u8>, map: &BTreeMap<String, String>) -> Result<()> {
    write_len(buf, map.len(), "map")?;
    for (k, v) in map {
        write_string(buf, k, "map key")?;
        write_string(buf, v, "map value")?;
    }
    Ok(())
}

fn write_peer_meta_map(buf: &mut Vec<u8>, map: &BTreeMap<String, PeerMeta>) -> Result<()> {
    write_len(buf, map.len(), "peer meta map")?;
    for (k, meta) in map {
        write_string(buf, k, "peer meta key")?;
        buf.push(if meta.optional { 1 } else { 0 });
    }
    Ok(())
}

fn write_string_list(buf: &mut Vec<u8>, list: &[String]) -> Result<()> {
    write_len(buf, list.len(), "list")?;
    for item in list {
        write_string(buf, item, "list item")?;
    }
    Ok(())
}

pub fn encode_current_binary(lf: &Lockfile) -> Result<Vec<u8>> {
    let mut packages_buf = Vec::with_capacity(4096);
    write_len(&mut packages_buf, lf.packages.len(), "package count")?;
    for (name, entry) in &lf.packages {
        write_string(&mut packages_buf, name, "package key")?;
        write_option_string(&mut packages_buf, &entry.version)?;
        write_option_string(&mut packages_buf, &entry.integrity)?;
        write_option_string(&mut packages_buf, &entry.resolved)?;
        write_string_map(&mut packages_buf, &entry.dependencies)?;
        write_string_map(&mut packages_buf, &entry.dev_dependencies)?;
        write_string_map(&mut packages_buf, &entry.optional_dependencies)?;
        write_string_map(&mut packages_buf, &entry.peer_dependencies)?;
        write_peer_meta_map(&mut packages_buf, &entry.peer_dependencies_meta)?;
        write_string_list(&mut packages_buf, &entry.os)?;
        write_string_list(&mut packages_buf, &entry.cpu_arch)?;
        write_option_string(&mut packages_buf, &entry.store_key)?;
        write_option_string(&mut packages_buf, &entry.content_hash)?;
        write_option_string(&mut packages_buf, &entry.link_mode)?;
        write_option_string(&mut packages_buf, &entry.store_path)?;
    }

    ensure!(packages_buf.len() <= MAX_LOCKFILE_SIZE, "lockfile data exceeds limit");

    let mut buf = Vec::with_capacity(LOCKFILE_MAGIC.len() + 16 + packages_buf.len());
    buf.extend_from_slice(LOCKFILE_MAGIC);
    write_u16(&mut buf, CURRENT_WIRE_VERSION);
    write_u16(&mut buf, 0); // reserved
    write_u32(&mut buf, lf.format);
    write_len(&mut buf, packages_buf.len(), "packages section")?;
    buf.extend_from_slice(&packages_buf);
    write_u32(&mut buf, 0); // reserved (extra section length)
    ensure!(buf.len() <= MAX_LOCKFILE_SIZE, "lockfile data exceeds limit");
    Ok(buf)
}

fn read_u16(data: &[u8], pos: &mut usize) -> anyhow::Result<u16> {
    let end = pos.checked_add(2).ok_or_else(|| anyhow!("overflow reading u16"))?;
    let slice = data.get(*pos..end).ok_or_else(|| anyhow!("unexpected eof reading u16"))?;
    *pos = end;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(data: &[u8], pos: &mut usize) -> anyhow::Result<u32> {
    let end = pos.checked_add(4).ok_or_else(|| anyhow!("overflow reading u32"))?;
    let slice = data.get(*pos..end).ok_or_else(|| anyhow!("unexpected eof reading u32"))?;
    *pos = end;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_len(data: &[u8], pos: &mut usize, what: &str) -> anyhow::Result<usize> {
    let value = read_u32(data, pos)?;
    usize::try_from(value).with_context(|| format!("{what} exceeds platform limits"))
}

fn read_exact<'a>(
    data: &'a [u8],
    pos: &mut usize,
    len: usize,
    what: &str,
) -> anyhow::Result<&'a [u8]> {
    let end = pos.checked_add(len).ok_or_else(|| anyhow!("length overflow reading {what}"))?;
    let slice = data.get(*pos..end).ok_or_else(|| anyhow!("unexpected eof reading {what}"))?;
    *pos = end;
    Ok(slice)
}

fn read_string(data: &[u8], pos: &mut usize, what: &str) -> anyhow::Result<String> {
    let len = read_len(data, pos, what)?;
    let bytes = read_exact(data, pos, len, what)?;
    Ok(std::str::from_utf8(bytes)
        .with_context(|| format!("{what} contains invalid utf-8"))?
        .to_owned())
}

fn read_option_string(data: &[u8], pos: &mut usize) -> anyhow::Result<Option<String>> {
    match data.get(*pos).copied() {
        Some(0) => {
            *pos += 1;
            Ok(None)
        }
        Some(1) => {
            *pos += 1;
            read_string(data, pos, "string").map(Some)
        }
        Some(other) => bail!("invalid option tag {other}"),
        None => bail!("unexpected eof reading option tag"),
    }
}

fn read_string_map(data: &[u8], pos: &mut usize) -> anyhow::Result<BTreeMap<String, String>> {
    let len = read_len(data, pos, "map length")?;
    let mut map = BTreeMap::new();
    for _ in 0..len {
        let key = read_string(data, pos, "map key")?;
        let value = read_string(data, pos, "map value")?;
        map.insert(key, value);
    }
    Ok(map)
}

fn read_peer_meta_map(data: &[u8], pos: &mut usize) -> anyhow::Result<BTreeMap<String, PeerMeta>> {
    let len = read_len(data, pos, "peer meta map length")?;
    let mut map = BTreeMap::new();
    for _ in 0..len {
        let key = read_string(data, pos, "peer meta key")?;
        let flag = data
            .get(*pos)
            .copied()
            .ok_or_else(|| anyhow!("unexpected eof reading peer meta flag"))?;
        *pos += 1;
        let optional = match flag {
            0 => false,
            1 => true,
            other => bail!("invalid peer meta flag {other}"),
        };
        map.insert(key, PeerMeta { optional });
    }
    Ok(map)
}

fn read_string_list(data: &[u8], pos: &mut usize) -> anyhow::Result<Vec<String>> {
    let len = read_len(data, pos, "list length")?;
    let mut list = Vec::with_capacity(len);
    for _ in 0..len {
        list.push(read_string(data, pos, "list item")?);
    }
    Ok(list)
}

fn parse_packages_section(
    packages_slice: &[u8],
    wire_version: u16,
) -> anyhow::Result<BTreeMap<String, PackageEntry>> {
    let mut packages_pos = 0usize;
    let package_count = read_len(packages_slice, &mut packages_pos, "package count")?;
    let mut packages = BTreeMap::new();
    for _ in 0..package_count {
        let key = read_string(packages_slice, &mut packages_pos, "package key")?;
        let version = read_option_string(packages_slice, &mut packages_pos)?;
        let integrity = read_option_string(packages_slice, &mut packages_pos)?;
        let resolved = read_option_string(packages_slice, &mut packages_pos)?;
        let dependencies = read_string_map(packages_slice, &mut packages_pos)?;
        let dev_dependencies = read_string_map(packages_slice, &mut packages_pos)?;
        let optional_dependencies = read_string_map(packages_slice, &mut packages_pos)?;
        let peer_dependencies = read_string_map(packages_slice, &mut packages_pos)?;
        let peer_dependencies_meta = read_peer_meta_map(packages_slice, &mut packages_pos)?;
        let (os, cpu_arch) = if wire_version >= 2 {
            let os = read_string_list(packages_slice, &mut packages_pos)?;
            let cpu_arch = read_string_list(packages_slice, &mut packages_pos)?;
            (os, cpu_arch)
        } else {
            (Vec::new(), Vec::new())
        };
        let (store_key, content_hash, link_mode, store_path) = if wire_version >= 3 {
            let store_key = read_option_string(packages_slice, &mut packages_pos)?;
            let content_hash = read_option_string(packages_slice, &mut packages_pos)?;
            let link_mode = read_option_string(packages_slice, &mut packages_pos)?;
            let store_path = read_option_string(packages_slice, &mut packages_pos)?;
            (store_key, content_hash, link_mode, store_path)
        } else {
            (None, None, None, None)
        };

        let entry = PackageEntry {
            version,
            integrity,
            resolved,
            dependencies,
            dev_dependencies,
            optional_dependencies,
            peer_dependencies,
            peer_dependencies_meta,
            os,
            cpu_arch,
            store_key,
            content_hash,
            link_mode,
            store_path,
        };
        packages.insert(key, entry);
    }

    ensure!(packages_pos == packages_slice.len(), "unexpected trailing data in packages section");

    Ok(packages)
}

pub fn decode_current_binary(data: &[u8]) -> anyhow::Result<Lockfile> {
    ensure!(data.len() <= MAX_LOCKFILE_SIZE, "lockfile exceeds maximum size");
    ensure!(data.starts_with(LOCKFILE_MAGIC), "missing lockfile magic header");

    let mut pos = LOCKFILE_MAGIC.len();
    let version = read_u16(data, &mut pos)?;
    if version != CURRENT_WIRE_VERSION && version != 2 && version != 1 {
        bail!("unsupported lockfile wire version {version}");
    }

    // Skip reserved field
    let _reserved = read_u16(data, &mut pos)?;

    let format = read_u32(data, &mut pos)?;

    let packages_section_len = read_len(data, &mut pos, "packages section length")?;
    let packages_section_start = pos;
    let packages_section_end = pos
        .checked_add(packages_section_len)
        .ok_or_else(|| anyhow!("packages section length overflow"))?;
    let packages_slice = data
        .get(packages_section_start..packages_section_end)
        .ok_or_else(|| anyhow!("unexpected eof reading packages section"))?;
    pos = packages_section_end;

    let packages = parse_packages_section(packages_slice, version)?;

    // Reserved section length (currently unused)
    let extras_len = read_len(data, &mut pos, "extras section length")?;
    let _extras = read_exact(data, &mut pos, extras_len, "extras section")?;
    ensure!(pos == data.len(), "unexpected trailing data");

    Ok(Lockfile { format, packages })
}

fn try_decode_standard<T>(data: &[u8]) -> Option<T>
where
    T: DeserializeOwned,
{
    let cfg = standard().with_limit::<MAX_LOCKFILE_SIZE>();
    std::panic::catch_unwind(|| decode_from_slice::<T, _>(data, cfg))
        .ok()
        .and_then(|res| res.ok().map(|(value, _)| value))
}

fn try_decode_legacy<T>(data: &[u8]) -> Option<T>
where
    T: DeserializeOwned,
{
    let cfg = legacy().with_limit::<MAX_LOCKFILE_SIZE>();
    std::panic::catch_unwind(|| decode_from_slice::<T, _>(data, cfg))
        .ok()
        .and_then(|res| res.ok().map(|(value, _)| value))
}

fn try_decode_v1_varint<T>(data: &[u8]) -> Option<T>
where
    T: DeserializeOwned,
{
    std::panic::catch_unwind(|| {
        bincode1::config::DefaultOptions::new()
            .with_limit(MAX_LOCKFILE_SIZE as u64)
            .allow_trailing_bytes()
            .deserialize::<T>(data)
    })
    .ok()
    .and_then(|res| res.ok())
}

fn try_decode_v1_fixint<T>(data: &[u8]) -> Option<T>
where
    T: DeserializeOwned,
{
    std::panic::catch_unwind(|| {
        bincode1::config::DefaultOptions::new()
            .with_fixint_encoding()
            .with_limit(MAX_LOCKFILE_SIZE as u64)
            .allow_trailing_bytes()
            .deserialize::<T>(data)
    })
    .ok()
    .and_then(|res| res.ok())
}

fn decode_manual_legacy(data: &[u8]) -> anyhow::Result<Lockfile> {
    fn read_varint(data: &[u8], pos: &mut usize) -> anyhow::Result<u64> {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte =
                *data.get(*pos).ok_or_else(|| anyhow::anyhow!("unexpected eof reading varint"))?;
            *pos += 1;
            value |= ((byte & 0x7F) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            anyhow::ensure!(shift < 64, "varint too large");
        }
    }

    fn read_u8(data: &[u8], pos: &mut usize) -> anyhow::Result<u8> {
        let byte = *data.get(*pos).ok_or_else(|| anyhow::anyhow!("unexpected eof reading byte"))?;
        *pos += 1;
        Ok(byte)
    }

    fn read_string(data: &[u8], pos: &mut usize) -> anyhow::Result<String> {
        let len = read_varint(data, pos)? as usize;
        let end = pos.checked_add(len).ok_or_else(|| anyhow::anyhow!("length overflow"))?;
        let slice =
            data.get(*pos..end).ok_or_else(|| anyhow::anyhow!("unexpected eof reading string"))?;
        *pos = end;
        Ok(std::str::from_utf8(slice)?.to_owned())
    }

    fn read_option_string(data: &[u8], pos: &mut usize) -> anyhow::Result<Option<String>> {
        match read_u8(data, pos)? {
            0 => Ok(None),
            1 => Ok(Some(read_string(data, pos)?)),
            other => anyhow::bail!("invalid option tag {other}"),
        }
    }

    fn read_string_map(data: &[u8], pos: &mut usize) -> anyhow::Result<BTreeMap<String, String>> {
        let mut map = BTreeMap::new();
        let len = read_varint(data, pos)? as usize;
        for _ in 0..len {
            let key = read_string(data, pos)?;
            let value = read_string(data, pos)?;
            map.insert(key, value);
        }
        Ok(map)
    }

    fn read_peer_meta_map(
        data: &[u8],
        pos: &mut usize,
    ) -> anyhow::Result<BTreeMap<String, PeerMeta>> {
        let mut map = BTreeMap::new();
        let len = read_varint(data, pos)? as usize;
        for _ in 0..len {
            let key = read_string(data, pos)?;
            let optional = match read_u8(data, pos)? {
                0 => false,
                1 => true,
                other => anyhow::bail!("invalid peer meta flag {other}"),
            };
            map.insert(key, PeerMeta { optional });
        }
        Ok(map)
    }

    let mut pos = 0usize;
    let format = read_varint(data, &mut pos)? as u32;
    let package_count = read_varint(data, &mut pos)? as usize;
    let mut packages = BTreeMap::new();
    for _ in 0..package_count {
        let key = read_string(data, &mut pos)?;
        let version = read_option_string(data, &mut pos)?;
        let integrity = read_option_string(data, &mut pos)?;
        let resolved = read_option_string(data, &mut pos)?;
        let dependencies = read_string_map(data, &mut pos)?;
        let dev_dependencies = read_string_map(data, &mut pos)?;
        let optional_dependencies = read_string_map(data, &mut pos)?;
        let peer_dependencies = read_string_map(data, &mut pos)?;
        let peer_dependencies_meta = read_peer_meta_map(data, &mut pos)?;
        let entry = PackageEntry {
            version,
            integrity,
            resolved,
            dependencies,
            dev_dependencies,
            optional_dependencies,
            peer_dependencies,
            peer_dependencies_meta,
            os: Vec::new(),
            cpu_arch: Vec::new(),
            store_key: None,
            content_hash: None,
            link_mode: None,
            store_path: None,
        };
        packages.insert(key, entry);
    }

    Ok(Lockfile { format, packages })
}

fn try_decode_previous_formats(data: &[u8]) -> Option<Lockfile> {
    if let Some(v) = try_decode_v1_varint::<Lockfile>(data) {
        return Some(v);
    }
    if let Some(legacy) = try_decode_v1_varint::<LegacyLockfile>(data) {
        return Some(legacy.into());
    }
    if let Some(v) = try_decode_v1_fixint::<Lockfile>(data) {
        return Some(v);
    }
    if let Some(legacy) = try_decode_v1_fixint::<LegacyLockfile>(data) {
        return Some(legacy.into());
    }
    if let Some(v) = try_decode_standard::<Lockfile>(data) {
        return Some(v);
    }
    if let Some(legacy) = try_decode_standard::<LegacyLockfile>(data) {
        return Some(legacy.into());
    }
    if let Some(legacy) = try_decode_legacy::<LegacyLockfile>(data) {
        return Some(legacy.into());
    }
    if let Some(v) = try_decode_legacy::<Lockfile>(data) {
        return Some(v);
    }
    if let Ok(manual) = decode_manual_legacy(data) {
        return Some(manual);
    }
    None
}

pub fn load(path: &PathBuf) -> Result<Lockfile> {
    let data = fs::read(path)?;
    let lf = if data.starts_with(LOCKFILE_MAGIC) {
        decode_current_binary(&data)?
    } else if let Some(decoded) = try_decode_previous_formats(&data) {
        decoded
    } else if let Ok(txt) = std::str::from_utf8(&data) {
        let trimmed = txt.trim_start();
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            serde_json::from_str(trimmed)?
        } else {
            bail!("unsupported lockfile format")
        }
    } else {
        bail!("unsupported lockfile format")
    };
    if lf.format == 0 {
        bail!("invalid lockfile format");
    }
    Ok(lf)
}

pub fn write(lf: &Lockfile, path: PathBuf) -> Result<()> {
    let data = encode_current_binary(lf)?;
    fs::write(path, data)?;
    Ok(())
}

/// Load a legacy JSON lockfile directly (compat migration helper)
pub fn load_json_compat(path: &PathBuf) -> Result<Lockfile> {
    let txt = fs::read_to_string(path)?;
    let lf: Lockfile = serde_json::from_str(&txt)?;
    if lf.format == 0 {
        anyhow::bail!("invalid lockfile format");
    }
    Ok(lf)
}

// Legacy compat structs for older bincode lockfiles (before dev/optional/peer fields)
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct LegacyPackageEntry {
    pub version: Option<String>,
    #[serde(default)]
    pub integrity: Option<String>,
    #[serde(default)]
    pub resolved: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies", skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependenciesMeta", skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_dependencies_meta: BTreeMap<String, PeerMeta>,
    #[serde(default)]
    pub os: Vec<String>,
    #[serde(default, rename = "cpu")]
    pub cpu_arch: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct LegacyLockfile {
    pub format: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, LegacyPackageEntry>,
}

impl From<LegacyLockfile> for Lockfile {
    fn from(old: LegacyLockfile) -> Self {
        let packages = old
            .packages
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    PackageEntry {
                        version: v.version,
                        integrity: v.integrity,
                        resolved: v.resolved,
                        dependencies: v.dependencies,
                        dev_dependencies: v.dev_dependencies,
                        optional_dependencies: v.optional_dependencies,
                        peer_dependencies: v.peer_dependencies,
                        peer_dependencies_meta: v.peer_dependencies_meta,
                        os: Vec::new(),
                        cpu_arch: Vec::new(),
                        store_key: None,
                        content_hash: None,
                        link_mode: None,
                        store_path: None,
                    },
                )
            })
            .collect();
        Lockfile { format: old.format, packages }
    }
}
