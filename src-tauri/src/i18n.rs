//! Translations of the text Rust shows the user (errors, notices, the tray menu, startup
//! dialogs, Discord messages). The English text in the source is the key: `tr!("Not found.")`.
//! `locales/<lang>.json` maps each key to its translation, or a plural key (`trn!`, keyed by the
//! English "other" form) to one string per CLDR plural category of that language. English has no
//! file. `scripts/check-i18n.mjs` checks every `tr!`/`trn!` key is in every catalog.
//!
//! Text is translated when it is made, in the language current then (`set`, from
//! `Settings.language`); text stored earlier (a crash's note) stays in its language.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::Deserialize;

/// Supported languages; index 0 (English) is the default.
pub const LANGS: [&str; 8] = ["en", "bs", "de", "id", "pl", "ru", "sv", "tr"];

static CURRENT: AtomicU8 = AtomicU8::new(0);

#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Text(String),
    Plural(HashMap<String, String>),
}

type Catalog = HashMap<String, Entry>;

fn source(i: usize) -> &'static str {
    match LANGS[i] {
        "bs" => include_str!("../locales/bs.json"),
        "de" => include_str!("../locales/de.json"),
        "id" => include_str!("../locales/id.json"),
        "pl" => include_str!("../locales/pl.json"),
        "ru" => include_str!("../locales/ru.json"),
        "sv" => include_str!("../locales/sv.json"),
        "tr" => include_str!("../locales/tr.json"),
        _ => "{}",
    }
}

fn catalog(i: usize) -> &'static Catalog {
    static CATALOGS: [OnceLock<Catalog>; LANGS.len()] = [const { OnceLock::new() }; LANGS.len()];
    CATALOGS[i].get_or_init(|| {
        serde_json::from_str(source(i)).unwrap_or_else(|e| {
            log::error!("locales/{}.json is broken: {e}", LANGS[i]);
            Catalog::new()
        })
    })
}

/// A Settings.language value ("" follows Windows' display language) to an index in `LANGS`.
fn resolve(pref: &str) -> usize {
    let pick = |tag: &str| {
        let base = tag.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
        LANGS.iter().position(|l| *l == base)
    };
    pick(pref).or_else(|| pick(&system_language())).unwrap_or(0)
}

/// Makes `pref` the language of all text made from now on.
pub fn set(pref: &str) {
    CURRENT.store(resolve(pref) as u8, Ordering::Relaxed);
}

/// The current language's code.
pub fn current() -> &'static str {
    LANGS[CURRENT.load(Ordering::Relaxed) as usize]
}

#[cfg(windows)]
fn system_language() -> String {
    use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    let mut buf = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, Some(&mut buf), 0) };
    String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)])
}

#[cfg(not(windows))]
fn system_language() -> String {
    String::new()
}

/// CLDR plural category of a whole number in `lang`.
fn category(lang: &str, n: u64) -> &'static str {
    let (d, h) = (n % 10, n % 100);
    let slavic_few = (2..=4).contains(&d) && !(12..=14).contains(&h);
    match lang {
        "id" => "other",
        "ru" if d == 1 && h != 11 => "one",
        "ru" if slavic_few => "few",
        "ru" => "many",
        "bs" if d == 1 && h != 11 => "one",
        "bs" if slavic_few => "few",
        "bs" => "other",
        "pl" if n == 1 => "one",
        "pl" if slavic_few => "few",
        "pl" => "many",
        _ if n == 1 => "one",
        _ => "other",
    }
}

fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = text.to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// `tr!`'s work: `key` in the current language, placeholders filled.
pub fn tr(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let i = CURRENT.load(Ordering::Relaxed) as usize;
    let text = match catalog(i).get(key) {
        Some(Entry::Text(t)) => t.as_str(),
        _ => key,
    };
    fill(text, args)
}

/// `trn!`'s work: the form of `other` (or `one`) for `n`, with `{n}` and the placeholders filled.
pub fn trn(n: u64, one: &'static str, other: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let i = CURRENT.load(Ordering::Relaxed) as usize;
    let cat = category(LANGS[i], n);
    let text = match catalog(i).get(other) {
        Some(Entry::Plural(forms)) => forms.get(cat).or_else(|| forms.get("other")).map_or(other, String::as_str),
        _ if cat == "one" => one,
        _ => other,
    };
    fill(&fill(text, &[("n", &n)]), args)
}

/// `tr!("Text with {name}.", name = value)`: the text in the current language.
macro_rules! tr {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::tr($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

/// `trn!(count, "{n} restart", "{n} restarts", more = value)`: the plural form for `count`.
macro_rules! trn {
    ($count:expr, $one:literal, $other:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::trn(($count) as u64, $one, $other, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_categories() {
        let forms = |lang| [0, 1, 2, 5, 11, 12, 21, 22, 25, 101, 111].map(|n| category(lang, n));
        assert_eq!(forms("en"), ["other", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other"]);
        assert_eq!(forms("ru"), ["many", "one", "few", "many", "many", "many", "one", "few", "many", "one", "many"]);
        assert_eq!(forms("pl"), ["many", "one", "few", "many", "many", "many", "many", "few", "many", "many", "many"]);
        assert_eq!(forms("bs"), ["other", "one", "few", "other", "other", "other", "one", "few", "other", "one", "other"]);
        assert!(forms("id").iter().all(|c| *c == "other"));
    }

    #[test]
    fn catalogs_parse_and_fill() {
        for i in 1..LANGS.len() {
            assert!(serde_json::from_str::<Catalog>(source(i)).is_ok(), "locales/{}.json", LANGS[i]);
        }
        assert_eq!(fill("{a} and {b}", &[("a", &1), ("b", &"x")]), "1 and x");
        assert_eq!(resolve("de"), 2);
    }
}
