// `sqlx::migrate!` embeds the migration files at compile time, but Cargo does
// not know about them. Without this, adding or editing a migration would not
// trigger a rebuild and the binary would silently ship stale migrations.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
