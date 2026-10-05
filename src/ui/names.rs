//! Champion names. On screen the game shows a champion's name through its own name reference
//! (`#asset/base/text/champion?description.<id>.name`), resolved in the game's language when
//! drawn - reading a label back gives the reference, not the name. So the overlay and the panel
//! write references, never names, and the grid's cards are named by champion id. This book only
//! maps the English names (`i18n`) and ids back to champion ids, for a card or text that shows
//! a name instead.

use std::collections::HashMap;

#[derive(Default)]
pub struct NameBook {
    by_text: HashMap<String, String>,
}

/// The game's reference to a champion's display name: a label given it shows the name in the
/// game's language.
pub fn name_ref(champion: &str) -> String {
    format!("#asset/base/text/champion?description.{champion}.name")
}

fn normalize(text: &str) -> String {
    text.trim().to_lowercase()
}

impl NameBook {
    /// The champion id a name belongs to.
    pub fn lookup(&self, text: &str) -> Option<&str> {
        self.by_text.get(&normalize(text)).map(String::as_str)
    }

    /// Adds a name (an English `i18n` name, a name reference, the id itself).
    pub fn learn(&mut self, text: &str, champion: &str) {
        if !text.is_empty() {
            self.by_text.entry(normalize(text)).or_insert_with(|| champion.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_references() {
        let mut book = NameBook::default();
        book.learn("Ahri", "league_ahri");
        book.learn(&name_ref("league_ahri"), "league_ahri");
        assert_eq!(book.lookup(" ahri "), Some("league_ahri"));
        assert_eq!(book.lookup("#asset/base/text/champion?description.league_ahri.name"), Some("league_ahri"));
        assert_eq!(book.lookup("Zed"), None);
    }
}
