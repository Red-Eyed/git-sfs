//! Makes the version injection in `src/version.rs` observable to Cargo.
//!
//! `option_env!` is evaluated at compile time, so without this the crate is not
//! rebuilt when `GIT_SFS_VERSION` changes and a release build can silently
//! carry the previous tag.

fn main() {
    println!("cargo::rerun-if-env-changed=GIT_SFS_VERSION");
}
