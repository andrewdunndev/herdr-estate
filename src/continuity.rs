//! sessions.log: the continuity record a campaign's resume reads.
//!
//! One line per conversation: an ISO-8601 UTC timestamp, the hostname
//! that opened it, and the conversation id, space-separated. Keep this
//! format stable: existing campaign directories depend on it.
//! herdr-estate owns continuity itself; herdr's own resume is never used.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn log_path(campaign_dir: &Path) -> PathBuf {
    campaign_dir.join("sessions.log")
}

pub fn read(campaign_dir: &Path) -> Vec<String> {
    let p = log_path(campaign_dir);
    if !p.is_file() {
        return Vec::new();
    }
    std::fs::read_to_string(&p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

pub fn latest_conversation(campaign_dir: &Path) -> Option<String> {
    read(campaign_dir)
        .last()
        .and_then(|l| l.split_whitespace().last())
        .map(str::to_string)
}

/// Civil (year, month, day) from a day count since the Unix epoch. A
/// well-known constant-time algorithm (Howard Hinnant's `civil_from_days`),
/// proleptic Gregorian, valid for every day this process will ever see.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

pub(crate) fn now_utc_iso() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let micros = dur.subsec_micros();
    let days = secs.div_euclid(86400);
    let secs_of_day = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let hh = secs_of_day / 3600;
    let mm = (secs_of_day % 3600) / 60;
    let ss = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{micros:06}+00:00")
}

/// The local hostname, via the `hostname` command — std has no
/// dependency-free `gethostname(2)` wrapper, and a `hostname`/`libc`
/// crate is outside this crate's dependency budget. Falls back to
/// `"unknown"` rather than failing continuity over a missing binary.
fn hostname() -> String {
    hostname_from("hostname")
}

/// `hostname` with the command to run as a parameter, so a test can
/// point it at a fake instead of the machine's own.
fn hostname_from(command: &str) -> String {
    std::process::Command::new(command)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn append(campaign_dir: &Path, conversation_id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(campaign_dir)?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(campaign_dir))?;
    writeln!(f, "{} {} {conversation_id}", now_utc_iso(), hostname())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    // A line in the SAME FORMAT an existing estate continuity log already
    // writes — timestamp, hostname, conversation id, space-separated —
    // copied for its shape only, not its content, so a pre-existing
    // sessions.log keeps resuming its latest conversation unmodified.
    const FIXTURE_LINE: &str =
        "2026-08-11T14:02:07.335219+00:00 example-host 3f9c2b7e-11aa-42bb-9cde-0123456789ab\n";

    #[test]
    fn no_file_reads_empty() {
        let d = TempDir::new();
        assert!(read(d.path()).is_empty());
        assert!(latest_conversation(d.path()).is_none());
    }

    #[test]
    fn append_then_read() {
        let d = TempDir::new();
        append(d.path(), "sid-one").unwrap();
        append(d.path(), "sid-two").unwrap();
        let lines = read(d.path());
        assert_eq!(lines.len(), 2);
        assert_eq!(latest_conversation(d.path()).as_deref(), Some("sid-two"));
    }

    #[test]
    fn line_shape_three_fields() {
        let d = TempDir::new();
        append(d.path(), "sid-one").unwrap();
        let line = &read(d.path())[0];
        let fields: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 3);
    }

    #[test]
    fn reads_a_preexisting_log_line() {
        let d = TempDir::new();
        std::fs::write(d.path().join("sessions.log"), FIXTURE_LINE).unwrap();
        assert_eq!(
            latest_conversation(d.path()).as_deref(),
            Some("3f9c2b7e-11aa-42bb-9cde-0123456789ab")
        );
    }

    #[test]
    fn appended_line_matches_fixture_shape() {
        let d = TempDir::new();
        append(d.path(), "new-sid").unwrap();
        let written = &read(d.path())[0];
        let fixture_fields: Vec<&str> = FIXTURE_LINE.split_whitespace().collect();
        let written_fields: Vec<&str> = written.split_whitespace().collect();
        assert_eq!(written_fields.len(), fixture_fields.len());
        // field 0: ISO-8601 timestamp
        let ts = written_fields[0];
        assert!(ts.len() >= 19, "timestamp too short: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        // field 2: the conversation id, verbatim
        assert_eq!(written_fields[2], "new-sid");
    }

    #[test]
    fn civil_from_days_epoch_and_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-09-21 is 20717 days after the epoch.
        assert_eq!(civil_from_days(20717), (2026, 9, 21));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn civil_from_days_across_century_and_leap_year_boundaries() {
        // (days since epoch, year, month, day), independently computed —
        // spans non-leap century years (1700/1800/1900/2100, not
        // divisible by 400), leap century years (1600/2000/2400, divisible
        // by 400), ordinary leap years, and the epoch and year-1 corners.
        // Operator-swap mutations in the arithmetic above (-/+/*// on any
        // one line) miss at least one of these.
        const CASES: &[(i64, i64, u32, u32)] = &[
            (-135140, 1600, 1, 1),
            (-135081, 1600, 2, 29),
            (-135080, 1600, 3, 1),
            (-98557, 1700, 2, 28),
            (-98556, 1700, 3, 1),
            (-62033, 1800, 2, 28),
            (-62032, 1800, 3, 1),
            (-25509, 1900, 2, 28),
            (-25508, 1900, 3, 1),
            (-672, 1968, 2, 29),
            (-671, 1968, 3, 1),
            (-1, 1969, 12, 31),
            (0, 1970, 1, 1),
            (1, 1970, 1, 2),
            (788, 1972, 2, 28),
            (789, 1972, 2, 29),
            (790, 1972, 3, 1),
            (10956, 1999, 12, 31),
            (10957, 2000, 1, 1),
            (11015, 2000, 2, 28),
            (11016, 2000, 2, 29),
            (11017, 2000, 3, 1),
            (12476, 2004, 2, 28),
            (12477, 2004, 2, 29),
            (12478, 2004, 3, 1),
            (19722, 2023, 12, 31),
            (19723, 2024, 1, 1),
            (19781, 2024, 2, 28),
            (19782, 2024, 2, 29),
            (19783, 2024, 3, 1),
            (20717, 2026, 9, 21),
            (24855, 2038, 1, 19),
            (47540, 2100, 2, 28),
            (47541, 2100, 3, 1),
            (157112, 2400, 2, 28),
            (157113, 2400, 2, 29),
            (157114, 2400, 3, 1),
            (-719162, 1, 1, 1),
            (-718798, 1, 12, 31),
            (-682945, 100, 2, 28),
            (-682944, 100, 3, 1),
            (-573373, 400, 2, 28),
            (-573372, 400, 2, 29),
            (-573371, 400, 3, 1),
            (-536848, 500, 2, 28),
            (-536847, 500, 3, 1),
            (18627, 2020, 12, 31),
            (18628, 2021, 1, 1),
            (29404, 2050, 7, 4),
            (-43664, 1850, 6, 15),
            (-719103, 1, 3, 1),
        ];
        for &(days, y, m, d) in CASES {
            assert_eq!(civil_from_days(days), (y, m, d), "days={days}");
        }
    }

    /// An executable at `dir/name` running `body` under `/bin/sh`.
    fn fake_command(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[test]
    fn hostname_is_the_commands_trimmed_output() {
        let d = TempDir::new();
        let cmd = fake_command(d.path(), "host", "printf '  test-host\\n'");
        assert_eq!(hostname_from(&cmd), "test-host");
    }

    #[test]
    fn a_failing_silent_or_missing_hostname_command_reads_as_unknown() {
        let d = TempDir::new();
        let failing = fake_command(d.path(), "failing", "echo test-host; exit 1");
        let silent = fake_command(d.path(), "silent", "printf '\\n'");
        let missing = d.path().join("missing").to_str().unwrap().to_string();
        for cmd in [failing, silent, missing] {
            assert_eq!(hostname_from(&cmd), "unknown", "{cmd}");
        }
    }

    #[test]
    fn an_appended_line_carries_the_hostname_between_time_and_id() {
        let d = TempDir::new();
        append(d.path(), "sid").unwrap();
        let written = &read(d.path())[0];
        let fields: Vec<&str> = written.split_whitespace().collect();
        assert_eq!(fields.len(), 3, "{written}");
        assert_eq!(fields[1], hostname());
        assert_eq!(fields[2], "sid");
    }
}
