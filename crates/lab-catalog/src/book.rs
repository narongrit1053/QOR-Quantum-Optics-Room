//! Vendor catalog books: user-supplied part indexes in `%APPDATA%\LabSetup\catalogs\*.json`
//! (format: `docs/CATALOG-FORMAT.md`). LabSetup ships no vendor data.

use std::path::Path;
use std::sync::OnceLock;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct BookItem {
    /// The file's top-level `"vendor"`, copied onto each item.
    #[serde(skip)]
    pub vendor: String,
    pub part: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub category: Vec<String>,
    pub url: String,
    #[serde(default)]
    pub cad_pdf: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
}

#[derive(serde::Deserialize)]
struct Book {
    vendor: String,
    items: Vec<BookItem>,
}

/// One catalog file's items, each carrying the file's vendor.
pub fn parse_book(text: &str) -> serde_json::Result<Vec<BookItem>> {
    let book: Book = serde_json::from_str(text)?;
    Ok(book.items.into_iter().map(|i| BookItem { vendor: book.vendor.clone(), ..i }).collect())
}

/// Every `*.json` in `dir` (sorted by name), de-duplicated by (vendor, part); unreadable or
/// malformed files are skipped with a message.
pub fn load_books(dir: &Path) -> Vec<BookItem> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
        .collect();
    files.sort();
    let mut seen = std::collections::HashSet::new();
    files
        .iter()
        .flat_map(|f| match std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|t| parse_book(&t).map_err(|e| e.to_string())) {
            Ok(items) => items,
            Err(error) => {
                eprintln!("catalog {} skipped: {error}", f.display());
                Vec::new()
            }
        })
        .filter(|i| seen.insert((i.vendor.clone(), i.part.clone())))
        .collect()
}

/// The user's catalogs from [`crate::catalogs_dir`], loaded once per session.
pub fn books() -> &'static [BookItem] {
    static BOOKS: OnceLock<Vec<BookItem>> = OnceLock::new();
    BOOKS.get_or_init(|| crate::catalogs_dir().map(|d| load_books(&d)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    const EXAMPLE: &str = include_str!("testdata/catalog-example.json");

    #[test]
    fn book_example_parses() {
        let book = super::parse_book(EXAMPLE).unwrap();
        assert_eq!(book.iter().map(|i| i.part.as_str()).collect::<Vec<_>>(), ["EX-KM1", "EX-P50", "EX-L100"]);
        assert!(book.iter().all(|i| i.vendor == "Example Optics" && !i.url.is_empty()));
        assert!(book[0].step.is_some() && book[1].step.is_none() && book[1].description.is_empty());
        assert!(super::parse_book(r#"{"vendor":"V","items":[{"part":"A","name":"B","category":[]}]}"#).is_err(), "url is required");
    }

    #[test]
    fn book_malformed_file_is_skipped() {
        let dir = std::env::temp_dir().join(format!("labsetup-books-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), EXAMPLE).unwrap();
        std::fs::write(dir.join("b.json"), "{ not json").unwrap();
        std::fs::write(dir.join("c.json"), EXAMPLE).unwrap(); // duplicates are dropped
        let items = super::load_books(&dir);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(items.len(), 3);
    }
}
