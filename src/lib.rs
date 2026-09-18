//! Focus tracking in the terminal: the data behind the sessions a person records.

pub mod app;
pub mod cli;
pub mod clock;
pub mod day;
pub mod duration;
pub mod export;
pub mod id;
pub mod liveness;
pub mod prefs;
pub mod session;
pub mod span;
pub mod stats;
pub mod store;
pub mod timer;
pub mod tree;
pub mod ui;

pub use app::run;

/// The application's own locale files, compiled in so an installed program carries its text
/// with it. Each is `(file name, contents)`, ready for the runtime or an [`qframe::i18n::I18n`].
#[must_use]
pub fn locales() -> [(&'static str, &'static str); 2] {
    [("en.toml", include_str!("../locales/en.toml")), ("tr.toml", include_str!("../locales/tr.toml"))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_locales_load_and_neither_misses_a_key_of_the_other() {
        let mut i18n = qframe::i18n::I18n::builtin();
        for (file, text) in locales() {
            assert!(i18n.add_source(file, text), "{file} did not load");
        }
        assert!(i18n.diagnostics().is_empty(), "{:?}", i18n.diagnostics());
        assert!(i18n.missing_keys("tr", "en").is_empty(), "missing in tr: {:?}", i18n.missing_keys("tr", "en"));
        assert!(i18n.missing_keys("en", "tr").is_empty(), "missing in en: {:?}", i18n.missing_keys("en", "tr"));
    }
}
