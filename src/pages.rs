//! Checking that rendered page components exist on disk.

use std::fmt;
use std::path::PathBuf;

/// Where page components live, and whether renders check for them.
#[derive(Debug, Clone)]
pub(crate) struct Pages {
    pub(crate) ensure_exist: bool,
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) extensions: Vec<String>,
}

impl Default for Pages {
    fn default() -> Self {
        Self {
            ensure_exist: false,
            paths: vec![PathBuf::from("resources/js/pages")],
            extensions: ["js", "jsx", "svelte", "ts", "tsx", "vue"].map(String::from).to_vec(),
        }
    }
}

impl Pages {
    /// Fail with [`ComponentNotFound`] when checks are on and no page file
    /// matches the component.
    pub(crate) fn ensure(&self, component: &str) -> Result<(), ComponentNotFound> {
        if !self.ensure_exist || self.exists(component) {
            return Ok(());
        }

        Err(ComponentNotFound {
            component: component.to_owned(),
        })
    }

    fn exists(&self, component: &str) -> bool {
        // Files are checked on every render, so pages added while the
        // server runs are found without a restart.
        self.paths.iter().any(|path| {
            self.extensions
                .iter()
                .any(|extension| path.join(format!("{component}.{extension}")).is_file())
        })
    }
}

/// A rendered page component has no file in the configured page paths.
///
/// Renders fail with it when [`Config::ensure_pages_exist`](crate::Config::ensure_pages_exist)
/// is on; it's the source of the [`PropError`](crate::PropError).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentNotFound {
    /// The component name, after any transformation.
    pub component: String,
}

impl fmt::Display for ComponentNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Inertia page component [{}] not found.", self.component)
    }
}

impl std::error::Error for ComponentNotFound {}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(dir: &str) -> Pages {
        let root = std::env::temp_dir().join(format!("inertia-pages-{dir}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Users")).unwrap();
        std::fs::write(root.join("Users/Index.tsx"), "").unwrap();

        Pages {
            ensure_exist: true,
            paths: vec![std::env::temp_dir().join("inertia-pages-nowhere"), root],
            ..Pages::default()
        }
    }

    #[test]
    fn finds_components_in_any_path_with_any_extension() {
        assert_eq!(pages("found").ensure("Users/Index"), Ok(()));
    }

    #[test]
    fn missing_components_fail() {
        let error = pages("missing").ensure("Users/Show").unwrap_err();

        assert_eq!(error.component, "Users/Show");
        assert_eq!(error.to_string(), "Inertia page component [Users/Show] not found.");
    }

    #[test]
    fn other_extensions_dont_count() {
        let pages = Pages {
            extensions: vec!["vue".into()],
            ..pages("extensions")
        };

        assert!(pages.ensure("Users/Index").is_err());
    }

    #[test]
    fn nothing_is_checked_unless_enabled() {
        assert_eq!(Pages::default().ensure("Anything"), Ok(()));
    }
}
