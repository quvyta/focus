# Contributing to qfocus

Thank you for taking the time. Bug reports, ideas, translations and pull requests are all welcome.

## Reporting a bug or asking for something

Open an issue at <https://github.com/quvyta/focus/issues> and pick the template that fits. For a bug, `qfocus --version`, your operating system and your terminal usually decide where the problem is, so the template asks for them. Your records are yours: before you attach a screenshot or a file from the data folder, check that it shows nothing you would rather keep private, such as the names of your focuses or your notes.

## Building and testing

The toolchain is pinned by `rust-toolchain.toml`; `rustup` picks it up by itself.

```sh
git clone https://github.com/quvyta/focus
cd focus
git config core.hooksPath .githooks   # once: formatting, clippy, tests and docs before every commit
cargo test
```

The tests write to temporary folders only; they never read or change your own records.

To try a change without touching your records either, give qfocus empty folders of its own (Linux and other Unix systems):

```sh
XDG_DATA_HOME=$(mktemp -d) XDG_CONFIG_HOME=$(mktemp -d) cargo run --bin qfocus
```

## Translations

qfocus speaks nine languages, and none of the translations has been read by a native speaker yet. If you speak one of them, reading it and telling us what sounds wrong is one of the most useful things you can do.

### Where the text lives

Each language is one file in `locales/`, named after its language code: `de.toml`, `pt-BR.toml`, `zh-Hans.toml`. English, `en.toml`, is the source every other file is checked against. A line looks like this:

```toml
[app]
cancelled = "Cancelled."
unsaved = { one = "{n} session could not be written and is kept in memory.", other = "{n} sessions could not be written and are kept in memory." }
```

A few words every Quvyta program shares, such as the key hints at the bottom of the window and the date picker, come from [quvyta-framework](https://github.com/quvyta/framework) and are translated there.

### Suggesting better wording, without building anything

Open an issue with the **Translation** template. Name the language, quote the line as it is now (the text on screen is enough; the key, such as `app.cancelled`, helps if you have it) and write the wording you would use. A short reason ("this is the word people use in time trackers") helps us keep the rest of the file consistent with it.

### Changing a language file

Edit the file and open a pull request. The tests check what is easy to break by accident:

- **Every key.** A translation has every key English has and no others. A missing key would show English in the middle of your language.
- **Placeholders.** Words in braces, such as `{n}`, `{focus}` or `{duration}`, are filled in by the program. Keep each one exactly as written, never translated; you may move it wherever the sentence needs it.
- **Plural forms.** A line with `one`, `other` and so on holds every form your language's plural rule can pick. Russian needs `one`, `few`, `many` and `other`; Japanese and Chinese need only `other`. The test names the form that is missing.
- **The command line help** (`[cli] usage`) fits an 80-column terminal on every line. Characters of Chinese and Japanese count as two columns.
- **Narrow windows.** Every screen is drawn at forty columns, and a translation may not cut more text short with `…` than English does on the same screen. If a word does not fit, a shorter word is almost always better than a shorter sentence around it.

Run the tests with `cargo test`. To read every screen of a language as the test draws it, pass its code:

```sh
QFOCUS_TOUR=de cargo test every_language_reads_whole_at_forty_columns -- --nocapture
```

To see it in the running program, choose the language under Settings, or start qfocus with that language and empty folders:

```sh
LANG=de_DE.UTF-8 XDG_DATA_HOME=$(mktemp -d) XDG_CONFIG_HOME=$(mktemp -d) cargo run --bin qfocus
```

### Adding a new language

1. Copy `locales/en.toml` to `locales/<code>.toml`, where `<code>` is the language's code: `it`, `pl`, or with a region or script where it matters, such as `pt-BR` or `zh-Hans`.
2. In its `[meta]` section, write the language's name in the language itself, the same code as the file name, and `fallback = "en"`:

   ```toml
   [meta]
   name = "Italiano"
   code = "it"
   fallback = "en"
   ```

3. Translate every line below `[meta]`.
4. Add the file to `locales()` in `src/lib.rs`. English stays first; the rest are in alphabetical order of their file names.
5. Add the language to the list under **Languages** in `README.md`.
6. Run `cargo test` and open a pull request.

A few guidelines keep the languages alike:

- Write the way the English does: short, plain, and speaking to the person as one would in your language's software (the familiar or the polite form, whichever is usual there). Say what happened and what to do next.
- Use one word for one thing throughout: *focus*, *category*, *session*, *break*, *goal* and *record* each keep the same translation in every screen.
- Leave the names `qfocus` and `Quvyta`, key names such as `ctrl+z` and `enter`, and the command line's words (`start`, `--json`) as they are.
- If you are unsure of a word, say so in the pull request; a question is better than a guess.

## Pull requests

- Keep one change per pull request, and say in the description what it changes for the person using qfocus.
- The commit hook must pass: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` and `cargo doc` without warnings. Please do not skip it.
- A bug fix comes with a test that fails without it.
- Text the person reads lives in `locales/`, never in the code. A new line goes into every language file, because the tests ask for every key in each; where you cannot translate it, copy the English text and say so in the pull request, and we will.
- Records are the person's data. A change must never lose or rewrite a record: records are only added to, and files are written in one piece or not at all.
- The interface comes from [quvyta-framework](https://github.com/quvyta/framework). A widget or behaviour every Quvyta application would need belongs there; open an issue in that repository first.

## Licence

By contributing you agree that your contribution is licensed under the MIT licence of this repository.
