//! Record which `Relink Package.vi` this build goes with.
//!
//! The VI ships next to the executable (dist-workspace.toml, wix/main.wxs)
//! instead of inside it, so only its hash is compiled in: lvpm uses it to
//! refuse a VI left over from another build.

use md5::{Digest, Md5};

const VI: &str = "src/lv-src/relink-package.vi";

fn main() {
    println!("cargo::rerun-if-changed={VI}");
    let bytes = std::fs::read(VI).unwrap_or_else(|e| panic!("cannot read {VI}: {e}"));
    let hex: String = Md5::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    println!("cargo::rustc-env=LVPM_RELINK_VI_MD5={hex}");
}
