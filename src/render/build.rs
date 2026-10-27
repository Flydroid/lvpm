//! The wording of `lvpm build`.

use lvpm_core::ops::build::Built;

/// What `lvpm build` prints once the package is written.
pub fn built(built: &Built) -> String {
    format!("built {} ({} files)\n", built.path.display(), built.files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_build_names_the_package_and_counts_its_files() {
        let b = Built {
            path: PathBuf::from("dist/demo-1.0.0.0.vip"),
            files: 4,
        };
        assert_eq!(built(&b), format!("built {} (4 files)\n", b.path.display()));
    }
}
