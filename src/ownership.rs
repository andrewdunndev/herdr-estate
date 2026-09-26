use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::paths::state_dir;

const FILE_NAME: &str = "owned.json";

fn path() -> PathBuf {
    state_dir().join(FILE_NAME)
}

fn load() -> BTreeMap<String, String> {
    let p = path();
    if !p.is_file() {
        return BTreeMap::new();
    }
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => return BTreeMap::new(),
    };
    serde_json::from_str::<BTreeMap<String, String>>(&text).unwrap_or_default()
}

fn temp_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{}-{n}-{nanos}", std::process::id())
}

fn save(records: &BTreeMap<String, String>) -> std::io::Result<()> {
    let p = path();
    let parent = p
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&parent)?;
    let tmp_path = parent.join(format!(".owned-{}.tmp", temp_suffix()));
    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp_path)?;
        let json = serde_json::to_vec(records)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        f.write_all(&json)?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp_path, &p) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(())
}

pub fn is_ours(workspace_id: &str) -> bool {
    load().contains_key(workspace_id)
}

pub fn ids() -> BTreeSet<String> {
    load().into_keys().collect()
}

pub fn record(address: &str, workspace_id: &str) -> std::io::Result<()> {
    let _held = crate::lock::hold_owned()?;
    let mut records = load();
    records.insert(workspace_id.to_string(), address.to_string());
    save(&records)
}

pub fn forget(workspace_id: &str) -> std::io::Result<()> {
    let _held = crate::lock::hold_owned()?;
    let mut records = load();
    if records.remove(workspace_id).is_some() {
        save(&records)?;
    }
    Ok(())
}

pub fn prune(
    existing_ids: &BTreeSet<String>,
    recorded_before_listing: &BTreeSet<String>,
) -> std::io::Result<()> {
    let _held = crate::lock::hold_owned()?;
    let records = load();
    let kept: BTreeMap<String, String> = records
        .iter()
        .filter(|(k, _)| {
            existing_ids.contains(k.as_str()) || !recorded_before_listing.contains(k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if kept != records {
        save(&kept)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    fn isolated() -> (TempDir, EnvGuard) {
        let d = TempDir::new();
        let g = EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            d.path().to_str().expect("utf8 path"),
        )]);
        (d, g)
    }

    #[test]
    fn unrecorded_workspace_is_not_ours() {
        let (_d, _g) = isolated();
        assert!(!is_ours("w1"));
    }

    #[test]
    fn record_then_is_ours() {
        let (_d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        assert!(is_ours("w1"));
    }

    #[test]
    fn record_persists_to_disk_at_the_exact_filename() {
        let (d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        let mut names: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort_unstable();
        assert_eq!(names, vec![FILE_NAME.to_string(), "owned.lock".to_string()]);
        let on_disk = std::fs::read_to_string(d.path().join(FILE_NAME)).unwrap();
        let parsed: BTreeMap<String, String> = serde_json::from_str(&on_disk).unwrap();
        assert_eq!(parsed.get("w1"), Some(&"home/garden".to_string()));
    }

    #[test]
    fn forget_drops_the_record() {
        let (_d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        forget("w1").unwrap();
        assert!(!is_ours("w1"));
    }

    #[test]
    fn forget_unrecorded_id_is_a_no_op() {
        let (_d, _g) = isolated();
        forget("never-recorded").unwrap();
    }

    #[test]
    fn cli_recorded_entry_is_visible_to_the_pickers_dir_resolution() {
        let home = TempDir::new();
        {
            let _g = EnvGuard::new(
                &[("HOME", home.path().to_str().expect("utf8 path"))],
                &["HERDR_PLUGIN_STATE_DIR", "XDG_STATE_HOME"],
            );
            record("home/garden", "w1").unwrap();
        }
        let picker_dir = home.path().join(".local/state/herdr/plugins/herdr-estate");
        let _g = EnvGuard::set(&[(
            "HERDR_PLUGIN_STATE_DIR",
            picker_dir.to_str().expect("utf8 path"),
        )]);
        assert!(is_ours("w1"));
    }

    #[test]
    fn prune_drops_ids_not_in_the_live_set() {
        let (_d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        record("home/taxes", "w2").unwrap();
        prune(&BTreeSet::from(["w2".to_string()]), &ids()).unwrap();
        assert!(!is_ours("w1"));
        assert!(is_ours("w2"));
    }

    #[test]
    fn prune_keeps_a_record_made_after_its_listing_was_taken() {
        let (_d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        let before = ids();
        record("home/attic", "w2").unwrap();
        prune(&BTreeSet::new(), &before).unwrap();
        assert!(!is_ours("w1"));
        assert!(is_ours("w2"));
    }

    #[test]
    fn prune_keeps_everything_when_all_still_exist() {
        let (_d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        prune(&BTreeSet::from(["w1".to_string()]), &ids()).unwrap();
        assert!(is_ours("w1"));
    }

    #[test]
    fn corrupt_file_reads_as_empty() {
        let (d, _g) = isolated();
        std::fs::create_dir_all(d.path()).unwrap();
        std::fs::write(d.path().join("owned.json"), "not json").unwrap();
        assert!(!is_ours("w1"));
    }

    #[test]
    fn non_object_json_reads_as_empty() {
        let (d, _g) = isolated();
        std::fs::create_dir_all(d.path()).unwrap();
        std::fs::write(d.path().join("owned.json"), "[1, 2, 3]").unwrap();
        assert!(!is_ours("w1"));
    }

    #[test]
    fn temp_suffix_is_unique_under_concurrent_calls() {
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};
        let results = Arc::new(Mutex::new(Vec::new()));
        let handles: Vec<_> = (0..50)
            .map(|_| {
                let results = Arc::clone(&results);
                std::thread::spawn(move || {
                    let s = temp_suffix();
                    results.lock().unwrap().push(s);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let values = results.lock().unwrap();
        let unique: HashSet<&String> = values.iter().collect();
        assert_eq!(
            unique.len(),
            values.len(),
            "duplicate temp_suffix values: {values:?}"
        );
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let (d, _g) = isolated();
        record("home/garden", "w1").unwrap();
        forget("w1").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".owned-"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
