//! The language registry.
//!
//! Java only, for now. TypeScript arrives in M5 to prove the [`Language`]
//! abstraction actually holds (SPEC.md §3).

mod java;

pub use java::JavaLanguage;

use std::path::Path;

use crate::language::Language;

static JAVA: JavaLanguage = JavaLanguage;

/// Every language this build supports.
static REGISTRY: &[&'static dyn Language] = &[&JAVA];

/// All registered languages.
#[must_use]
pub fn all() -> &'static [&'static dyn Language] {
    REGISTRY
}

/// Look up a language by the name it reports from [`Language::name`].
#[must_use]
pub fn by_name(name: &str) -> Option<&'static dyn Language> {
    REGISTRY.iter().copied().find(|l| l.name() == name)
}

/// Pick a language for a path, by file extension.
///
/// The extension is matched case-insensitively: Windows and case-insensitive
/// macOS filesystems both produce `Foo.JAVA` often enough to matter, and a merge
/// driver that silently declines to handle a file because of its case would be
/// an unpleasant surprise.
#[must_use]
pub fn detect(path: &Path) -> Option<&'static dyn Language> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    REGISTRY
        .iter()
        .copied()
        .find(|l| l.file_extensions().contains(&ext.as_str()))
}
