//! The wording of `lvpm publish`.

use lvpm_core::ops::publish::Published;

/// What `lvpm publish` prints once the package is in the feed.
pub fn published(published: &Published) -> String {
    format!("published {}\n", published.path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_publish_names_the_file_it_wrote() {
        let p = Published {
            path: PathBuf::from("demo-1.0.0.0.vip"),
        };
        assert_eq!(published(&p), "published demo-1.0.0.0.vip\n");
    }
}
