fn main() {
    // sqlx::migrate! embeds the migration directory into the crate at compile time.
    // Tell Cargo to invalidate morrows-store when a migration is added or changed.
    println!("cargo:rerun-if-changed=migrations");
}
