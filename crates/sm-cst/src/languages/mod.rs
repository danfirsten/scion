//! The language registry.
//!
//! Java and TypeScript. TypeScript was pulled forward from M5 to prove the
//! [`Language`] abstraction actually holds (SPEC.md §3), and it is registered
//! twice — once per `tree-sitter-typescript` grammar — because `.ts` and `.tsx`
//! genuinely need different parsers. See [`TypeScriptLanguage`] for why that is
//! one parameterised type rather than two.

mod java;
mod typescript;

pub use java::JavaLanguage;
pub use typescript::{TsDialect, TypeScriptLanguage};

use std::path::Path;

use crate::language::Language;

static JAVA: JavaLanguage = JavaLanguage;
static TYPESCRIPT: TypeScriptLanguage = TypeScriptLanguage::TYPESCRIPT;
static TSX: TypeScriptLanguage = TypeScriptLanguage::TSX;

/// Every language this build supports.
///
/// Order matters only in that [`detect`] takes the first entry claiming an
/// extension; the extension sets are disjoint, and a test asserts they stay that
/// way.
static REGISTRY: &[&'static dyn Language] = &[&JAVA, &TYPESCRIPT, &TSX];

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
