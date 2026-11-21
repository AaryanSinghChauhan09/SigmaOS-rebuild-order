//! Pacman sync database reader for rebuild-order computation.
//!
//! Reads `<dbpath>/sync/<repo>.db*` tar archives (plain/gzip/zstd) and extracts
//! per-package dependency metadata from each `desc` entry: NAME, BASE,
//! DEPENDS, MAKEDEPENDS, CHECKDEPENDS, PROVIDES.

use std::{
    collections::HashSet,
    io::Read,
    path::{Path, PathBuf},
};

use crate::error::RebuildError;

/// Dependency fields extracted from a sync db `desc` entry.
#[derive(Debug, Clone, Default)]
pub struct SyncPkg {
    /// Binary package name (`%NAME%`).
    pub name: String,
    /// Package base (`%BASE%`, falls back to name).
    pub base: String,
    /// Runtime dependencies (`%DEPENDS%`).
    pub depends: Vec<String>,
    /// Build dependencies (`%MAKEDEPENDS%`).
    pub makedepends: Vec<String>,
    /// Check dependencies (`%CHECKDEPENDS%`).
    pub checkdepends: Vec<String>,
    /// Provided virtual names (`%PROVIDES%` first column before `=`).
    pub provides: Vec<String>,
}

impl SyncPkg {
    /// The package base used for lookups.
    pub fn pkgbase(&self) -> &str {
        if self.base.is_empty() {
            &self.name
        } else {
            &self.base
        }
    }
}

/// All packages across the given sync databases.
#[derive(Debug, Default)]
pub struct SyncDb {
    /// Packages keyed by binary name.
    pub packages: Vec<SyncPkg>,
    /// Quick lookup by name.
    pub by_name: std::collections::HashMap<String, usize>,
}

impl SyncDb {
    /// Load all sync dbs for `repos` under `db_path`.
    pub fn load(db_path: &Path, repos: &[String]) -> Result<Self, RebuildError> {
        let sync_dir = db_path.join("sync");
        let mut db = SyncDb::default();
        if !sync_dir.is_dir() {
            return Err(RebuildError::PacmanDb(format!(
                "{}: missing sync dir",
                sync_dir.display()
            )));
        }
        for repo in repos {
            for candidate in sync_db_candidates(&sync_dir, repo) {
                db.load_file(&candidate)?;
            }
        }
        db.reindex();
        Ok(db)
    }

    /// Load from explicit db file paths.
    pub fn load_files(files: &[PathBuf]) -> Result<Self, RebuildError> {
        let mut db = SyncDb::default();
        for f in files {
            db.load_file(f)?;
        }
        db.reindex();
        Ok(db)
    }

    fn load_file(&mut self, path: &Path) -> Result<(), RebuildError> {
        let data = std::fs::read(path)
            .map_err(|e| RebuildError::PacmanDb(format!("{}: {e}", path.display())))?;
        let tar_bytes = decompress(&data).ok_or_else(|| {
            RebuildError::PacmanDb(format!("{}: unsupported compression", path.display()))
        })?;
        let mut archive = tar::Archive::new(tar_bytes.as_slice());
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if !path.ends_with("desc") {
                continue;
            }
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            if let Some(pkg) = parse_desc(&text) {
                self.packages.push(pkg);
            }
        }
        Ok(())
    }

    fn reindex(&mut self) {
        self.by_name.clear();
        for (i, p) in self.packages.iter().enumerate() {
            self.by_name.insert(p.name.clone(), i);
        }
    }

    /// Find a package by name, pkgbase, or provided virtual name.
    pub fn find(&self, pkgname: &str) -> Option<&SyncPkg> {
        if let Some(&i) = self.by_name.get(pkgname) {
            return self.packages.get(i);
        }
        self.packages
            .iter()
            .find(|p| p.pkgbase() == pkgname || p.provides.iter().any(|v| v == pkgname))
    }
}

fn is_marker(line: &str) -> bool {
    line.len() > 2 && line.starts_with('%') && line.ends_with('%')
}

/// Parse one sync db `desc` entry into a [`SyncPkg`].
fn parse_desc(text: &str) -> Option<SyncPkg> {
    let mut fields: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if is_marker(line) {
            let name = line.trim_matches('%').to_string();
            fields.entry(name.clone()).or_default();
            current = Some(name);
        } else if let Some(field) = &current {
            fields.get_mut(field).unwrap().push(line.trim().to_string());
        }
    }
    let get = |name: &str| -> Vec<String> {
        fields
            .get(name)
            .map(|v| {
                v.iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    };
    let first = |name: &str| -> String {
        get(name).into_iter().next().unwrap_or_default()
    };
    let name = first("NAME");
    if name.is_empty() {
        return None;
    }
    Some(SyncPkg {
        name,
        base: first("BASE"),
        depends: get("DEPENDS"),
        makedepends: get("MAKEDEPENDS"),
        checkdepends: get("CHECKDEPENDS"),
        provides: get("PROVIDES")
            .iter()
            .map(|p| p.split('=').next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    })
}

/// Candidate sync db paths for a repo.
fn sync_db_candidates(sync_dir: &Path, repo: &str) -> Vec<PathBuf> {
    ["", ".tar.gz", ".tar.zst", ".tar.xz", ".gz"]
        .iter()
        .map(|suffix| sync_dir.join(format!("{repo}.db{suffix}")))
        .filter(|p| p.is_file())
        .collect()
}

/// Decompress sync db bytes based on magic bytes.
fn decompress(data: &[u8]) -> Option<Vec<u8>> {
    if data.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return zstd::stream::decode_all(data).ok().map(|v| v.to_vec());
    }
    if data.starts_with(&[0x1F, 0x8B]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(data).read_to_end(&mut out).ok()?;
        return Some(out);
    }
    if data.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]) {
        return None;
    }
    Some(data.to_vec())
}

/// Strip version constraints and modifiers from a dependency spec.
///
/// `gcc-libs>=10` → `gcc-libs`; `so:libc.so.6` → `so:libc.so.6` (the prefix
/// is part of the alpm dep name); `foo=1.0-1` → `foo`.
pub fn dep_name(spec: &str) -> &str {
    let bytes = spec.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if matches!(b, b'>' | b'<' | b'=' | b'~' | b' ' | b'\t') {
            return &spec[..i];
        }
    }
    spec
}

/// Collect all dependency names of a package (depends + makedepends,
/// optionally checkdepends), mapped to the dep spec's package name.
pub fn dep_names(pkg: &SyncPkg, with_check: bool) -> Vec<String> {
    let mut names: Vec<String> = pkg
        .depends
        .iter()
        .chain(pkg.makedepends.iter())
        .map(|d| dep_name(d).to_string())
        .collect();
    if with_check {
        names.extend(pkg.checkdepends.iter().map(|d| dep_name(d).to_string()));
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dep_name_strips_version() {
        assert_eq!(dep_name("gcc-libs>=10"), "gcc-libs");
        assert_eq!(dep_name("foo=1.0-1"), "foo");
        assert_eq!(dep_name("bar~2"), "bar");
        assert_eq!(dep_name("plain"), "plain");
        assert_eq!(dep_name("so:libc.so.6"), "so:libc.so.6");
    }

    #[test]
    fn test_parse_desc() {
        let text = "%NAME%\nfoo\n%BASE%\nfoo\n%VERSION%\n1-1\n%DEPENDS%\nbar>=2\nbaz\n%MAKEDEPENDS%\nqux\n%PROVIDES%\nfoo=1\n";
        let pkg = parse_desc(text).unwrap();
        assert_eq!(pkg.name, "foo");
        assert_eq!(pkg.depends, vec!["bar>=2", "baz"]);
        assert_eq!(pkg.makedepends, vec!["qux"]);
        assert_eq!(pkg.provides, vec!["foo"]);
    }

    #[test]
    fn test_syncdb_load_tar() {
        let dir = tempfile::tempdir().unwrap();
        let mut tar_data = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_data);
            let desc = "%NAME%\nfoo\n%BASE%\nfoo\n%DEPENDS%\nbar\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(desc.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "extra/foo-1-1/desc", desc.as_bytes()).unwrap();
            b.finish().unwrap();
        }
        std::fs::create_dir_all(dir.path().join("sync")).unwrap();
        std::fs::write(dir.path().join("sync/extra.db"), &tar_data).unwrap();
        let db = SyncDb::load(dir.path(), &["extra".to_string()]).unwrap();
        let foo = db.find("foo").unwrap();
        assert_eq!(foo.depends, vec!["bar"]);
    }
}
