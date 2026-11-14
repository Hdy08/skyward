use anyhow::{anyhow, bail, Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
pub mod modprobe;

/// Where a module source's files are in the recovery: the vendor_boot
/// ramdisk's modules under /lib/modules, the vendor_dlkm and vendor
/// partitions where they are mounted, and the kernel's own modules in
/// system_dlkm, which it also keeps in one folder.
pub fn source_dir(source: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(match source {
        "vendor_boot" => "/lib/modules",
        "vendor_dlkm" => "/vendor_dlkm/lib/modules",
        "vendor" => "/vendor/lib/modules",
        "system_dlkm" => return Ok(system_modules(Path::new("/system_dlkm"))),
        _ => bail!("unknown module source {source:?}"),
    }))
}

fn system_modules(root: &Path) -> PathBuf {
    let direct = root.join("lib/modules");
    if direct.is_dir() { direct } else { root.join("flatten/lib/modules") }
}

/// Load `names` (module names without `.ko`) with their dependencies. Each
/// comes from the first of `dirs` that holds it, and its dependencies from
/// that directory's modules.dep; a module the kernel already has counts as
/// loaded. Returns the files loaded, dependencies first.
pub fn load(dirs: &[PathBuf], names: &[String]) -> Result<Vec<PathBuf>> {
    let mut loaded = Vec::new();
    for name in names {
        let file = format!("{name}.ko");
        let dir = dirs
            .iter()
            .find(|d| d.join(&file).is_file())
            .ok_or_else(|| anyhow!("{file} is in none of {dirs:?}"))?;
        let graph = parse_dep(dir)?;
        for m in graph.load_order(std::slice::from_ref(&file)) {
            let path = graph
                .resolve_from(&m, dirs)
                .ok_or_else(|| anyhow!("{m} is listed in {}/modules.dep but missing from all module sources", dir.display()))?;
            insmod(&path, "")?;
            loaded.push(path);
        }
    }
    Ok(loaded)
}

/// A `modules.dep` graph: each module file mapped to the module files it
/// depends on. Keys and values are reduced to bare file names so the graph is
/// independent of where the modules were sourced from (GKI devices keep them
/// in several places: vendor_boot ramdisk, vendor_dlkm, vendor).
#[derive(Debug, Default)]
pub struct ModuleGraph {
    pub base: PathBuf,
    pub deps: BTreeMap<String, Vec<String>>,
}

pub fn parse_dep(base: impl AsRef<Path>) -> Result<ModuleGraph> {
    let base = base.as_ref().to_path_buf();
    let dep_path = base.join("modules.dep");
    let text = fs::read_to_string(&dep_path)
        .with_context(|| format!("read {}", dep_path.display()))?;
    let mut deps = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((lhs, rhs)) = line.split_once(':') else {
            continue;
        };
        let name = basename(lhs.trim());
        let ds = rhs.split_whitespace().map(basename).collect();
        deps.insert(name, ds);
    }
    Ok(ModuleGraph { base, deps })
}

fn basename(p: &str) -> String {
    Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

impl ModuleGraph {
    /// Vendor modules.dep also names dependencies shipped in system_dlkm.
    /// Keep this graph's copy first, then look in the configured sources.
    fn resolve_from(&self, module: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
        self.resolve_path(module).or_else(|| dirs.iter().filter(|dir| **dir != self.base).find_map(|dir| {
            let direct = dir.join(module);
            if direct.is_file() { Some(direct) } else { find_file(dir, module) }
        }))
    }

    /// Topologically ordered load list. With an empty `requested`, every module
    /// in the graph is scheduled; otherwise only the requested modules and
    /// their transitive dependencies, dependencies first.
    pub fn load_order(&self, requested: &[String]) -> Vec<String> {
        let mut order = Vec::new();
        let mut seen = BTreeSet::new();
        let list: Vec<String> = if requested.is_empty() {
            self.deps.keys().cloned().collect()
        } else {
            requested.to_vec()
        };
        for m in list {
            self.visit(&m, &mut seen, &mut order);
        }
        order
    }

    fn visit(&self, m: &str, seen: &mut BTreeSet<String>, order: &mut Vec<String>) {
        if !seen.insert(m.to_string()) {
            return;
        }
        if let Some(ds) = self.deps.get(m) {
            for d in ds {
                self.visit(d, seen, order);
            }
        }
        order.push(m.to_string());
    }

    pub fn resolve_path(&self, module: &str) -> Option<PathBuf> {
        let direct = self.base.join(module);
        if direct.exists() {
            return Some(direct);
        }
        find_file(&self.base, module)
    }
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().map(|s| s == name).unwrap_or(false) {
            return Some(p);
        }
    }
    None
}

/// Hand the open module `file` to the kernel with the settings `params`.
#[cfg(target_os = "android")]
fn finit_module(file: &fs::File, params: &str) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let params = std::ffi::CString::new(params).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let rc = unsafe { libc::syscall(libc::SYS_finit_module, file.as_raw_fd(), params.as_ptr(), 0 as libc::c_int) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn finit_module(_file: &fs::File, _params: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "modules are loaded only on the phone"))
}

pub fn insmod(path: impl AsRef<Path>, params: &str) -> Result<()> {
    let path = path.as_ref();
    let f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    match finit_module(&f, params) {
        Ok(()) => Ok(()),
        // Already loaded is success for our purposes.
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => Ok(()),
        Err(e) => Err(e).with_context(|| format!("finit_module {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_dependencies_first() {
        let mut deps = BTreeMap::new();
        deps.insert("a.ko".to_string(), vec!["b.ko".to_string()]);
        deps.insert("b.ko".to_string(), vec!["c.ko".to_string()]);
        deps.insert("c.ko".to_string(), vec![]);
        let g = ModuleGraph { base: PathBuf::new(), deps };
        let order = g.load_order(&["a.ko".to_string()]);
        assert_eq!(order, vec!["c.ko", "b.ko", "a.ko"]);
    }

    #[test]
    fn strips_paths_to_basenames() {
        assert_eq!(basename("kernel/drivers/foo/bar.ko"), "bar.ko");
    }

    #[test]
    fn vendor_dependencies_can_be_shipped_in_system_dlkm() {
        let root = tempfile::tempdir().unwrap();
        let vendor = root.path().join("vendor");
        let system = root.path().join("system/lib/modules");
        fs::create_dir_all(&vendor).unwrap();
        fs::create_dir_all(&system).unwrap();
        fs::write(vendor.join("modules.dep"), "cfg80211.ko: /system/lib/modules/rfkill.ko\n").unwrap();
        fs::write(vendor.join("cfg80211.ko"), []).unwrap();
        fs::write(system.join("rfkill.ko"), []).unwrap();
        let graph = parse_dep(&vendor).unwrap();
        let dirs = [vendor.clone(), system.clone()];
        let paths: Vec<_> = graph.load_order(&["cfg80211.ko".into()]).iter().map(|m| graph.resolve_from(m, &dirs).unwrap()).collect();
        assert_eq!(paths, [system.join("rfkill.ko"), vendor.join("cfg80211.ko")]);
        fs::write(vendor.join("rfkill.ko"), []).unwrap();
        assert_eq!(graph.resolve_from("rfkill.ko", &dirs), Some(vendor.join("rfkill.ko")), "prefer the requesting graph's copy");
    }

    #[test]
    fn system_dlkm_can_keep_modules_directly_or_under_flatten() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("flatten/lib/modules")).unwrap();
        assert_eq!(system_modules(root.path()), root.path().join("flatten/lib/modules"));
        fs::create_dir_all(root.path().join("lib/modules")).unwrap();
        assert_eq!(system_modules(root.path()), root.path().join("lib/modules"));
    }
}
