//! Every language file carries all of the English text, in the same shape.
//!
//! A key missing from a translation would show English in the middle of another language, and a
//! placeholder dropped or misspelt would print `{name}` literally or lose a number, so each file
//! is compared with English key by key and placeholder by placeholder. Plural tables must hold
//! every form their language's plural rule can pick, so Russian's "few" and "many" never fall
//! back to "other" without anyone noticing.

use std::collections::{BTreeMap, BTreeSet};

use qframe::i18n::{I18n, PluralCategory};
use toml::de::{DeTable, DeValue};

use crate::locales;

/// One message of a language file: plain text, or text by plural form.
#[derive(Debug)]
enum Text {
    Plain(String),
    Plural(BTreeMap<String, String>),
}

/// The messages of a language file by dotted key, `[meta]` left out, and its code.
fn messages(file: &str, text: &str) -> (String, BTreeMap<String, Text>) {
    let root = DeTable::parse(text).unwrap_or_else(|error| panic!("{file} is not TOML: {error}"));
    let mut out = BTreeMap::new();
    let mut code = String::new();
    for (section, value) in root.get_ref() {
        let DeValue::Table(table) = value.get_ref() else {
            panic!("{file}: `{}` is not a section", section.get_ref());
        };
        if section.get_ref() == "meta" {
            if let Some(DeValue::String(value)) = table.get("code").map(toml::Spanned::get_ref) {
                code = value.to_string();
            }
            continue;
        }
        flatten(file, section.get_ref(), table, &mut out);
    }
    (code, out)
}

fn flatten(file: &str, prefix: &str, table: &DeTable<'_>, out: &mut BTreeMap<String, Text>) {
    for (key, value) in table {
        let full = format!("{prefix}.{}", key.get_ref());
        match value.get_ref() {
            DeValue::String(text) => {
                out.insert(full, Text::Plain(text.to_string()));
            }
            DeValue::Table(inner) if inner.keys().all(|form| PluralCategory::from_name(form.get_ref()).is_some()) => {
                let forms = inner
                    .iter()
                    .map(|(form, text)| match text.get_ref() {
                        DeValue::String(text) => (form.get_ref().to_string(), text.to_string()),
                        _ => panic!("{file}: `{full}.{}` is not text", form.get_ref()),
                    })
                    .collect();
                out.insert(full, Text::Plural(forms));
            }
            DeValue::Table(inner) => flatten(file, &full, inner, out),
            _ => panic!("{file}: `{full}` is neither text nor a table"),
        }
    }
}

/// The `{placeholder}` names in `text`.
fn placeholders(text: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        names.insert(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    names
}

/// The placeholders of a message, whatever its shape: those of its `other` form for a plural.
fn placeholders_of(text: &Text) -> BTreeSet<String> {
    match text {
        Text::Plain(text) => placeholders(text),
        Text::Plural(forms) => forms.get("other").map(|text| placeholders(text)).unwrap_or_default(),
    }
}

/// The language a code names, as the plural rules know it: `pt` for `pt`, `zh` for `zh`.
fn language(code: &str) -> &str {
    code.split(['-', '_']).next().unwrap_or(code)
}

#[test]
fn every_language_file_loads_and_names_itself_after_its_file() {
    let mut i18n = I18n::builtin();
    for (file, text) in locales() {
        assert!(i18n.add_source(file, text), "{file} did not load");
        let (code, _) = messages(file, text);
        assert_eq!(format!("{code}.toml"), *file, "the code in [meta] names the file");
    }
    assert!(i18n.diagnostics().is_empty(), "{:?}", i18n.diagnostics());
    assert_eq!(locales()[0].0, "en.toml", "English is the reference and comes first");
}

#[test]
fn every_language_has_every_english_key_and_no_other() {
    let (english_file, english_text) = locales()[0];
    let (_, english) = messages(english_file, english_text);
    for (file, text) in &locales()[1..] {
        let (_, translated) = messages(file, text);
        let missing: Vec<&String> = english.keys().filter(|key| !translated.contains_key(*key)).collect();
        let extra: Vec<&String> = translated.keys().filter(|key| !english.contains_key(*key)).collect();
        assert!(missing.is_empty(), "missing in {file}: {missing:?}");
        assert!(extra.is_empty(), "only in {file}: {extra:?}");
    }
}

#[test]
fn every_translation_keeps_the_placeholders_and_the_shape_of_the_english() {
    let (english_file, english_text) = locales()[0];
    let (_, english) = messages(english_file, english_text);
    for (file, text) in &locales()[1..] {
        let (_, translated) = messages(file, text);
        for (key, source) in &english {
            let Some(target) = translated.get(key) else { continue };
            let wanted = placeholders_of(source);
            match (source, target) {
                (Text::Plain(_), Text::Plain(text)) => {
                    assert_eq!(placeholders(text), wanted, "{file}: `{key}` = {text:?}");
                }
                (Text::Plural(_), Text::Plural(forms)) => {
                    for (form, text) in forms {
                        assert_eq!(placeholders(text), wanted, "{file}: `{key}.{form}` = {text:?}");
                    }
                }
                _ => panic!("{file}: `{key}` is a plural in one file and plain text in the other"),
            }
        }
    }
}

#[test]
fn every_plural_holds_each_form_its_language_can_pick() {
    for (file, text) in locales() {
        let (code, messages) = messages(file, text);
        let needed: BTreeSet<&str> =
            (0..=1_000).map(|n| PluralCategory::of(language(&code), n).name()).chain(["other"]).collect();
        for (key, message) in &messages {
            if let Text::Plural(forms) = message {
                let held: BTreeSet<&str> = forms.keys().map(String::as_str).collect();
                let missing: Vec<&&str> = needed.difference(&held).collect();
                assert!(missing.is_empty(), "{file}: `{key}` lacks {missing:?}");
            }
        }
    }
}

#[test]
fn the_checks_catch_what_they_are_for() {
    let english = "[meta]\nname = \"English\"\ncode = \"en\"\n[a]\nhello = \"Hi {name}\"\nn = { one = \"{n} file\", other = \"{n} files\" }\n";
    let (_, english) = messages("en.toml", english);
    let russian = "[meta]\nname = \"Русский\"\ncode = \"ru\"\n[a]\nhello = \"Привет {nmae}\"\nn = { one = \"{n} файл\", other = \"{n} файла\" }\n";
    let (code, russian) = messages("ru.toml", russian);
    assert_eq!(code, "ru");
    assert_ne!(placeholders_of(&russian["a.hello"]), placeholders_of(&english["a.hello"]), "a misspelt name is caught");
    let Text::Plural(forms) = &russian["a.n"] else { panic!("a plural reads as a plural") };
    assert!(!forms.contains_key("few"), "a Russian table without `few` is what the plural check refuses");
    assert_eq!(PluralCategory::of(language("ru"), 3).name(), "few");
    assert_eq!(language("zh-Hans"), "zh");
}

#[test]
fn the_command_line_help_fits_a_standard_terminal_in_every_language() {
    for (file, text) in locales() {
        let (_, messages) = messages(file, text);
        let Some(Text::Plain(usage)) = messages.get("cli.usage") else { panic!("{file}: no usage") };
        for line in usage.lines() {
            assert!(qframe::text::width(line) <= 80, "{file}: the help line is wider than 80 cells: {line}");
        }
    }
}

#[test]
fn the_system_setting_picks_each_language_even_where_its_code_names_a_region_or_a_script() {
    let mut i18n = I18n::builtin();
    for (file, text) in locales() {
        assert!(i18n.add_source(file, text), "{file} did not load");
    }
    let cases = [
        ("pt_BR.UTF-8", "pt-BR"),
        ("pt_PT.UTF-8", "pt-BR"),
        ("zh_CN.UTF-8", "zh-Hans"),
        ("zh_SG.UTF-8", "zh-Hans"),
        ("ja_JP.UTF-8", "ja"),
        ("de_AT.UTF-8", "de"),
        ("fr_CA.UTF-8", "fr"),
        ("es_MX.UTF-8", "es"),
        ("ru_RU.UTF-8", "ru"),
        ("tr_TR.UTF-8", "tr"),
        ("en_GB.UTF-8", "en"),
    ];
    for (lang, code) in cases {
        let found = i18n.detect(|name| (name == "LANG").then(|| lang.to_owned()));
        assert_eq!(found.as_deref(), Some(code), "LANG={lang}");
    }
}
