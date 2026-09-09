use super::*;
use std::path::{Component, Path, PathBuf};

#[cfg(target_os = "linux")]
pub(super) fn detect() -> io::Result<Snapshot> {
    let groups = std::fs::read_to_string("/proc/self/cgroup")?;
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")?;
    if let Some(bytes) = cgroup_limit(&groups, &mounts, |path| {
        match std::fs::read_to_string(path) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    })? {
        return Ok(Snapshot {
            bytes,
            source: "ui.memory_source_cgroup",
        });
    }
    let info = std::fs::read_to_string("/proc/meminfo")?;
    let bytes =
        mem_available(&info).ok_or_else(|| io::Error::other("MemAvailable missing or invalid"))?;
    Ok(Snapshot {
        bytes,
        source: "ui.memory_source_available",
    })
}

fn mem_available(info: &str) -> Option<u64> {
    info.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != "MemAvailable:" {
            return None;
        }
        let kib = parts.next()?.parse::<u64>().ok()?;
        if parts.next()? != "kB" {
            return None;
        }
        kib.checked_mul(1024)
    })
}

fn unescape(path: &str) -> PathBuf {
    PathBuf::from(
        path.replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

/// Use the tightest finite ancestor limit, not remaining cgroup usage. The
/// hierarchy may be mounted at a nonstandard location or delegated subtree.
fn cgroup_limit(
    groups: &str,
    mounts: &str,
    mut read: impl FnMut(&Path) -> io::Result<Option<String>>,
) -> io::Result<Option<u64>> {
    let mut smallest = None;
    for line in mounts.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            continue;
        };
        let before: Vec<_> = before.split_whitespace().collect();
        let after: Vec<_> = after.split_whitespace().collect();
        if before.len() < 5 || after.len() < 3 {
            continue;
        }
        let unified = after[0] == "cgroup2";
        if !unified && !(after[0] == "cgroup" && after[2].split(',').any(|value| value == "memory"))
        {
            continue;
        }
        for group in groups.lines() {
            let parts: Vec<_> = group.splitn(3, ':').collect();
            if parts.len() != 3
                || if unified {
                    !parts[1].is_empty()
                } else {
                    !parts[1].split(',').any(|value| value == "memory")
                }
            {
                continue;
            }
            let membership = Path::new(parts[2]);
            if !membership.is_absolute()
                || membership
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
            {
                return Err(io::Error::other("invalid cgroup membership path"));
            }
            let root = unescape(before[3]);
            let mount = unescape(before[4]);
            let relative = match membership.strip_prefix(&root) {
                Ok(relative) => relative,
                Err(_) if membership == Path::new("/") => Path::new(""),
                Err(_) => continue,
            };
            let mut current = mount.join(relative);
            loop {
                let file = current.join(if unified {
                    "memory.max"
                } else {
                    "memory.limit_in_bytes"
                });
                if let Some(value) = read(&file)? {
                    let value = value.trim();
                    if value != "max" && value != "-1" {
                        let bytes = value
                            .parse::<u64>()
                            .map_err(|_| io::Error::other("invalid cgroup memory limit"))?;
                        // v1 represents unlimited with a page-aligned LONG_MAX.
                        if unified || bytes < (1u64 << 60) {
                            smallest = Some(smallest.map_or(bytes, |old: u64| old.min(bytes)));
                        }
                    }
                }
                if current == mount || !current.pop() {
                    break;
                }
            }
        }
    }
    Ok(smallest)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limit(groups: &str, mounts: &str, files: &[(&str, &str)]) -> Option<u64> {
        cgroup_limit(groups, mounts, |path| {
            Ok(files
                .iter()
                .find(|(name, _)| Path::new(name) == path)
                .map(|(_, value)| value.to_string()))
        })
        .unwrap()
    }
    #[test]
    fn v2_uses_finite_parent_total_even_when_leaf_is_unlimited() {
        assert_eq!(
            limit(
                "0::/parent/child",
                "1 0 0:1 / /cg rw - cgroup2 cgroup rw",
                &[
                    ("/cg/parent/child/memory.max", "max"),
                    ("/cg/parent/memory.max", "2147483648"),
                    ("/cg/memory.max", "4294967296"),
                ]
            ),
            Some(2147483648)
        );
    }
    #[test]
    fn v1_handles_delegated_mounts_and_unlimited_sentinel() {
        assert_eq!(
            limit(
                "5:cpu,memory:/tenant/game",
                "1 0 0:1 /tenant /custom\\040cg rw - cgroup cgroup rw,memory",
                &[
                    ("/custom cg/game/memory.limit_in_bytes", "1073741824"),
                    ("/custom cg/memory.limit_in_bytes", "9223372036854771712"),
                ]
            ),
            Some(1073741824)
        );
        assert_eq!(
            limit(
                "5:memory:/",
                "1 0 0:1 / /cg rw - cgroup cgroup rw,memory",
                &[("/cg/memory.limit_in_bytes", "9223372036854771712"),]
            ),
            None
        );
    }
    #[test]
    fn namespace_root_and_missing_or_unlimited_limits() {
        assert_eq!(
            limit(
                "0::/",
                "1 0 0:1 /tenant /cg rw - cgroup2 cgroup rw",
                &[("/cg/memory.max", "1024"),]
            ),
            Some(1024)
        );
        assert_eq!(
            limit(
                "0::/",
                "1 0 0:1 / /cg rw - cgroup2 cgroup rw",
                &[("/cg/memory.max", "max")]
            ),
            None
        );
        assert_eq!(limit("0::/", "", &[]), None);
        assert_eq!(
            mem_available("MemTotal: 9000 kB\nMemFree: 100 kB\nMemAvailable: 4000 kB"),
            Some(4096000)
        );
    }
}
