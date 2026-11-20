//! Thorlabs store-search requests, response parsing and the query plan for a full update.
//! Mirrors `scripts/catalog/fetch-thorlabs.py`; no network code lives here.

use std::collections::BTreeSet;

use crate::book::BookItem;

pub const SECTIONS: [&str; 3] = ["Optomechanical Components", "Fiber", "Optical Elements"];
pub const EXCLUDE: [&str; 4] = ["Enclosure Systems", "Instrumentation Shelves", "Optomechanics Kits", "Optomechanics Capabilities"];
pub const PAGE_SIZE: usize = 200;
pub const SEARCH_CAP: usize = 10_000;
pub const GRAPHQL_URL: &str = "https://www.thorlabs.com/graphql";

const QUERY: &str = r#"query($q:String,$n:Int,$a:String){ products(storeId:"Thorlabs-Website", cultureName:"en-US",
 query:$q, first:$n, after:$a){ totalCount items { code descriptions { content reviewType } assets { name url group } imgSrc images { url name } } } }"#;

/// The search response no longer has the expected shape.
#[derive(Debug, PartialEq, Eq)]
pub struct FormatChanged(pub String);

pub struct SearchPage {
    pub total: usize,
    pub items: Vec<BookItem>,
    /// Second-level categories of the kept items (index 1 of their category path).
    pub second_levels: BTreeSet<String>,
}

pub fn request_body(query: &str, offset: usize) -> String {
    serde_json::json!({"query": QUERY, "variables": {"q": query, "n": PAGE_SIZE, "a": offset.to_string()}}).to_string()
}

/// `{query slug}-{offset:06}.json`, the script's cache file name.
pub fn cache_name(query: &str, offset: usize) -> String {
    let mut slug = String::new();
    for c in query.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    format!("{slug}-{offset:06}.json")
}

fn strip(s: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => text.push(c),
            _ => {}
        }
    }
    // shortcut: only the common entities, not all of HTML's; upgrade if titles show others.
    let text = text.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_image(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("").to_lowercase();
    [".jpg", ".jpeg", ".png", ".webp"].iter().any(|e| path.ends_with(e))
}

/// A thumb/small named image, else `imgSrc`, else the first image file.
fn pick_image(item: &serde_json::Value) -> Option<String> {
    let images: Vec<(&str, String)> = item["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i["url"].as_str()?, i["name"].as_str().unwrap_or("").to_lowercase())))
        .collect();
    let thumb = images.iter().find(|(_, n)| (n.contains("thumb") || n.contains("small")) && is_image(n)).map(|i| i.0);
    let src = item["imgSrc"].as_str().filter(|u| is_image(u));
    thumb.or(src).or_else(|| images.iter().find(|(u, _)| is_image(u)).map(|i| i.0)).map(str::to_string)
}

fn description<'a>(it: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    it["descriptions"].as_array()?.iter().find(|x| x["reviewType"] == k).and_then(|x| x["content"].as_str()).filter(|c| !c.is_empty())
}

/// Top-down category names from the Breadcrumb: leaf-first [name, path] pairs ending "All Products", item.
fn breadcrumb(it: &serde_json::Value) -> Option<Vec<String>> {
    let pairs: Vec<Vec<String>> = serde_json::from_str(description(it, "Breadcrumb")?).ok()?;
    let mut crumbs: Vec<String> = pairs.into_iter().filter_map(|p| p.into_iter().next()).collect();
    crumbs.truncate(crumbs.len().saturating_sub(2));
    crumbs.reverse();
    Some(crumbs)
}

fn to_item(it: &serde_json::Value, crumbs: Vec<String>, section: &str, exclude: &[&str]) -> Option<BookItem> {
    let code = it["code"].as_str()?;
    let d = |k: &str| description(it, k);
    let at = crumbs.iter().position(|c| c == section)?;
    if crumbs.iter().any(|c| exclude.contains(&c.as_str())) {
        return None;
    }
    let assets: Vec<&serde_json::Value> = it["assets"].as_array().into_iter().flatten().collect();
    let url_of = |a: &&serde_json::Value| a["url"].as_str().map(str::to_string);
    let name_of = |a: &serde_json::Value| a["name"].as_str().unwrap_or("").to_lowercase();
    let step = assets.iter().find(|a| a["group"] == "Step").and_then(url_of).or_else(|| {
        assets.iter().find(|a| [".step", ".stp"].iter().any(|e| name_of(a).ends_with(e))).and_then(url_of)
    });
    let pdf = assets
        .iter()
        .find(|a| {
            let g = a["group"].as_str().unwrap_or("");
            (g.contains("CAD") || g.contains("Drawing")) && name_of(a).ends_with(".pdf")
        })
        .and_then(url_of);
    let title = strip(d("Title").unwrap_or(""));
    let description = match d("FamilyPageName") {
        Some(f) => strip(&format!("{f}: {}", d("Title").unwrap_or(""))),
        None => title.clone(),
    };
    Some(BookItem {
        vendor: "Thorlabs".into(),
        part: code.into(),
        name: if title.is_empty() { code.into() } else { title },
        description,
        category: crumbs[at..].to_vec(),
        url: format!("https://www.thorlabs.com{}", d("ItemUrl").map(str::to_string).unwrap_or_else(|| format!("/item/{code}"))),
        cad_pdf: pdf,
        step,
        image: pick_image(it),
    })
}

/// The items of one response page that belong to `section`; `Err` when the response is not the known shape.
pub fn parse_page(json: &str, section: &str, exclude: &[&str]) -> Result<SearchPage, FormatChanged> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| FormatChanged(e.to_string()))?;
    let products = &v["data"]["products"];
    let total = products["totalCount"].as_u64().ok_or_else(|| FormatChanged("data.products.totalCount missing".into()))? as usize;
    let raw = products["items"].as_array().ok_or_else(|| FormatChanged("data.products.items missing".into()))?;
    let crumbs: Vec<Option<Vec<String>>> = raw.iter().map(breadcrumb).collect();
    // Skipping one odd item is fine; no parseable Breadcrumb on a whole page means the shape changed.
    if !raw.is_empty() && crumbs.iter().all(Option::is_none) {
        return Err(FormatChanged("no item has a usable Breadcrumb".into()));
    }
    let items: Vec<BookItem> = raw.iter().zip(crumbs).filter_map(|(i, c)| to_item(i, c?, section, exclude)).collect();
    let second_levels = items.iter().filter_map(|i| i.category.get(1).cloned()).collect();
    Ok(SearchPage { total, items, second_levels })
}

pub struct Request {
    pub section: String,
    pub query: String,
    pub offset: usize,
}

/// Which page to fetch next: each section's own query, then (over the search cap) its second-level categories.
pub struct Planner {
    sections: Vec<String>,
    sec: usize,
    /// The current section's queries; the first is the section itself.
    queries: Vec<String>,
    qi: usize,
    offset: usize,
    total: usize,
    second: BTreeSet<String>,
}

impl Planner {
    pub fn new(sections: &[&str]) -> Planner {
        let sections: Vec<String> = sections.iter().map(|s| s.to_string()).collect();
        let queries = sections.first().cloned().into_iter().collect();
        Planner { sections, sec: 0, queries, qi: 0, offset: 0, total: 0, second: BTreeSet::new() }
    }

    pub fn next(&self) -> Option<Request> {
        Some(Request { section: self.sections.get(self.sec)?.clone(), query: self.queries.get(self.qi)?.clone(), offset: self.offset })
    }

    /// Record the page fetched for `next()` and advance.
    pub fn accept(&mut self, page: &SearchPage) {
        if self.next().is_none() {
            return;
        }
        self.total = page.total;
        self.second.extend(page.second_levels.iter().cloned());
        self.offset += PAGE_SIZE;
        if self.offset < page.total.min(SEARCH_CAP) {
            return;
        }
        if self.qi == 0 && page.total > SEARCH_CAP {
            let own = self.sections[self.sec].clone();
            self.queries.extend(self.second.iter().filter(|s| **s != own).cloned());
        }
        self.qi += 1;
        self.offset = 0;
        self.total = 0;
        if self.qi >= self.queries.len() {
            self.sec += 1;
            self.qi = 0;
            self.second.clear();
            self.queries = self.sections.get(self.sec).cloned().into_iter().collect();
        }
    }

    /// Section being fetched, and done / total of its current query.
    pub fn progress(&self) -> (String, usize, usize) {
        let section = self.sections.get(self.sec).or(self.sections.last()).cloned().unwrap_or_default();
        (section, self.offset.min(self.total), self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("testdata/thorlabs-search-page.json");

    #[test]
    fn parses_fixture() {
        let p = parse_page(FIXTURE, "Fiber", &EXCLUDE).unwrap();
        assert_eq!(p.total, 3);
        assert_eq!(p.items.len(), 1); // one in an excluded category, one outside the section
        assert_eq!(p.items[0].category, ["Fiber", "Fiber Components", "EX Couplers"]);
        assert_eq!(p.items[0].step.as_deref(), Some("https://example.com/ex-fc1.step"));
        assert_eq!(p.items[0].cad_pdf.as_deref(), Some("https://example.com/ex-fc1.pdf"));
        assert_eq!(p.items[0].image.as_deref(), Some("https://example.com/ex-fc1_thumb.jpg"));
        assert_eq!(p.items[0].name, "Example Coupler & Mount, 1 in");
        assert_eq!(p.items[0].description, "Example Fiber Coupler: Example Coupler & Mount, 1 in");
        assert_eq!((p.items[0].vendor.as_str(), p.items[0].url.as_str()), ("Thorlabs", "https://www.thorlabs.com/item/EX-FC1"));
        assert!(p.second_levels.contains("Fiber Components"));
    }

    fn page_with(img_src: serde_json::Value, images: serde_json::Value) -> String {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        v["data"]["products"]["items"][0]["imgSrc"] = img_src;
        v["data"]["products"]["items"][0]["images"] = images;
        v.to_string()
    }

    #[test]
    fn image_falls_back_to_imgsrc_then_first_image() {
        let image = |json: String| parse_page(&json, "Fiber", &EXCLUDE).unwrap().items[0].image.clone();
        let big = serde_json::json!([{"url": "https://example.com/a.gif", "name": "a.gif"}, {"url": "https://example.com/b.png", "name": "b.png"}]);
        assert_eq!(image(page_with("https://example.com/s.jpg?w=1".into(), big.clone())).as_deref(), Some("https://example.com/s.jpg?w=1"));
        assert_eq!(image(page_with("https://example.com/s.pdf".into(), big.clone())).as_deref(), Some("https://example.com/b.png"));
        assert_eq!(image(page_with(serde_json::Value::Null, serde_json::json!([]))), None);
    }

    #[test]
    fn unusable_breadcrumbs_are_format_changed_but_other_sections_are_not() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        for it in v["data"]["products"]["items"].as_array_mut().unwrap() {
            for d in it["descriptions"].as_array_mut().unwrap() {
                if d["reviewType"] == "Breadcrumb" {
                    d["content"] = "Fiber > Couplers".into();
                }
            }
        }
        assert_eq!(parse_page(&v.to_string(), "Fiber", &EXCLUDE).err(), Some(FormatChanged("no item has a usable Breadcrumb".into())));
        let p = parse_page(FIXTURE, "Nothing Here", &EXCLUDE).unwrap();
        assert_eq!((p.total, p.items.len()), (3, 0));
    }

    #[test]
    fn missing_products_is_format_changed() {
        assert!(parse_page(r#"{"data":{}}"#, "Fiber", &[]).is_err());
        assert!(parse_page(r#"{"data":{"products":{"totalCount":1}}}"#, "Fiber", &[]).is_err());
    }

    fn page(total: usize, second: &[&str]) -> SearchPage {
        SearchPage { total, items: vec![], second_levels: second.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn planner_pages_and_expands_over_cap() {
        let mut pl = Planner::new(&["Fiber", "Optical Elements"]);
        let mut seen = vec![];
        while let Some(r) = pl.next() {
            seen.push((r.query.clone(), r.offset));
            let big = r.query == "Fiber";
            pl.accept(&if big { page(10_400, &["Zed", "Alpha"]) } else { page(250, &[]) });
        }
        let fiber: Vec<_> = seen.iter().filter(|s| s.0 == "Fiber").collect();
        assert_eq!(fiber.len(), 50);
        assert_eq!((fiber[0].1, fiber[49].1), (0, 9_800));
        let tail: Vec<_> = seen[50..].iter().map(|s| (s.0.as_str(), s.1)).collect();
        assert_eq!(tail, [("Alpha", 0), ("Alpha", 200), ("Zed", 0), ("Zed", 200), ("Optical Elements", 0), ("Optical Elements", 200)]);
    }

    #[test]
    fn planner_progress() {
        let mut pl = Planner::new(&["Fiber"]);
        assert_eq!(pl.progress(), ("Fiber".to_string(), 0, 0));
        pl.accept(&page(500, &[]));
        assert_eq!(pl.progress(), ("Fiber".to_string(), 200, 500));
    }

    #[test]
    fn cache_name_matches_script() {
        assert_eq!(cache_name("Optical Elements", 400), "optical-elements-000400.json");
        assert!(request_body("Fiber", 400).contains(r#""a":"400""#));
    }
}
