//! bin entrypoint. Argument parsing and verb dispatch (`resume`, `fresh`,
//! `list`) land here on top of the `herdr_estate` library; this stub
//! exists so the crate builds and runs end to end while that dispatch is
//! still being ported.

fn main() {
    println!("usage: herdr-estate <resume|fresh|list> [address] [--dry-run]");
}
