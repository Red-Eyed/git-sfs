//! Makes generated binary inputs observable to Cargo.
//!
//! `option_env!` is evaluated at compile time, so without this the crate is not
//! rebuilt when `GIT_SFS_VERSION` changes and a release build can silently
//! carry the previous tag. The embedded `llms.txt` is generated from docs, so a
//! regenerated reference should also rebuild the binary.

fn main() {
    println!("cargo::rerun-if-env-changed=GIT_SFS_VERSION");
    println!("cargo::rerun-if-changed=../../llms.txt");
}
