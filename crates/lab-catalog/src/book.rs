//! Vendor catalog books: user-supplied part indexes in `%APPDATA%\LabSetup\catalogs\*.json`
//! (format: `docs/CATALOG-FORMAT.md`). LabSetup ships no vendor data.

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
}
