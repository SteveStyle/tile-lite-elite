fn main() {
    // `sqlx::migrate!` embeds each *existing* migration file via `include_str!`,
    // so the compiler notices edits to files it already knows about. A brand
    // new file added to `migrations/` has no such reference anywhere, so
    // nothing tells Cargo to re-run the macro when one shows up — without
    // this, a newly added migration could silently sit unembedded until some
    // unrelated source change forced a recompile.
    println!("cargo:rerun-if-changed=migrations");
}
