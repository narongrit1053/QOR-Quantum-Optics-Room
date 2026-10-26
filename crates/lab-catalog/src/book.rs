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

/// The embedded Thorlabs optomechanics book (include_str! of data/thorlabs-optomechanics.json), parsed once.
pub fn thorlabs() -> &'static [BookItem] {
    static BOOK: OnceLock<Vec<BookItem>> = OnceLock::new();
    BOOK.get_or_init(|| {
        serde_json::from_str::<Book>(include_str!("../data/thorlabs-optomechanics.json"))
            .expect("embedded Thorlabs book is valid JSON")
            .items
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
        let n = book.iter().filter(|i| i.image.is_some()).count();
        assert!(n * 2 > book.len(), "{n} of {} items have an image", book.len());
    }
}
