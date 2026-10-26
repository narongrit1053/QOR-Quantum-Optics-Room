//! Vendor catalog books: machine-readable part indexes harvested by `scripts/catalog/`.

use std::sync::OnceLock;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct BookItem {
    pub part: String,
    pub name: String,
    pub description: String,
    pub category: Vec<String>,
    pub url: String,
    pub cad_pdf: Option<String>,
    pub step: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(serde::Deserialize)]
struct Book {
    items: Vec<BookItem>,
}

/// The embedded Thorlabs books (optomechanics, then optical elements), parsed once and de-duplicated by part.
pub fn thorlabs() -> &'static [BookItem] {
    static BOOK: OnceLock<Vec<BookItem>> = OnceLock::new();
    BOOK.get_or_init(|| {
        let mut seen = std::collections::HashSet::new();
        [
            include_str!("../data/thorlabs-optomechanics.json"),
            include_str!("../data/thorlabs-optical-elements.json"),
        ]
        .into_iter()
        .flat_map(|s| serde_json::from_str::<Book>(s).expect("embedded Thorlabs book is valid JSON").items)
        .filter(|i| seen.insert(i.part.clone()))
        .collect()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn book_thorlabs_loads() {
        let book = super::thorlabs();
        assert!(book.len() > 1000, "{} items", book.len());
        for i in book {
            assert!(!i.part.is_empty() && !i.url.is_empty(), "{i:?}");
            for c in &i.category {
                let c = c.to_lowercase();
                assert!(!c.contains("kit") && !c.contains("suppl"), "{c}");
            }
        }
        for p in ["KM100", "SM1L05"] {
            let i = book.iter().find(|i| i.part == p).unwrap_or_else(|| panic!("{p} missing"));
            assert!(i.step.is_some(), "{p} has no STEP");
        }
        assert!(book.iter().find(|i| i.part == "KM100").unwrap().image.is_some(), "KM100 has no image");
        assert!(book.iter().any(|i| i.category[0] == "Optical Elements"), "no optical elements");
        let n = book.iter().filter(|i| i.image.is_some()).count();
        assert!(n * 2 > book.len(), "{n} of {} items have an image", book.len());
    }
}
