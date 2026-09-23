//! Rebuild embedded migrations when the migration directory changes.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
