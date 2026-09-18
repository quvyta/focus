//! Focus tracking in the terminal: the data behind the sessions a person records.

pub mod app;
pub mod cli;
pub mod clock;
pub mod config;
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
/// English comes first: it is the language every other file is checked against.
#[must_use]
pub fn locales() -> &'static [(&'static str, &'static str)] {
    &[
        ("en.toml", include_str!("../locales/en.toml")),
        ("de.toml", include_str!("../locales/de.toml")),
        ("es.toml", include_str!("../locales/es.toml")),
        ("fr.toml", include_str!("../locales/fr.toml")),
        ("ja.toml", include_str!("../locales/ja.toml")),
        ("pt-BR.toml", include_str!("../locales/pt-BR.toml")),
        ("ru.toml", include_str!("../locales/ru.toml")),
        ("tr.toml", include_str!("../locales/tr.toml")),
        ("zh-Hans.toml", include_str!("../locales/zh-Hans.toml")),
    ]
}

#[cfg(test)]
mod languages;
