//! SQLite catalog database: JSON catalog files are imported into it and the Catalog window
//! searches it (docs/superpowers/specs/2026-11-21-catalog-database-design.md). It is a
//! rebuildable cache; a schema version mismatch deletes and recreates the file.

use std::path::Path;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use crate::book::{parse_book, BookItem};

const SCHEMA_VERSION: i64 = 1;
const JSON_PREFIX: &str = "json:";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS items(
    vendor TEXT NOT NULL, part TEXT NOT NULL, name TEXT NOT NULL, description TEXT NOT NULL,
    category_json TEXT NOT NULL, cat1 TEXT NOT NULL, cat2 TEXT NOT NULL, url TEXT NOT NULL,
    step TEXT, cad_pdf TEXT, image TEXT, source TEXT NOT NULL, winner INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(vendor, part, source));
CREATE INDEX IF NOT EXISTS items_winner_cat2 ON items(winner, cat2);
CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
    part, name, description, content='items', content_rowid='rowid', tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS items_ai AFTER INSERT ON items BEGIN
    INSERT INTO items_fts(rowid, part, name, description) VALUES (new.rowid, new.part, new.name, new.description);
END;
CREATE TRIGGER IF NOT EXISTS items_ad AFTER DELETE ON items BEGIN
    INSERT INTO items_fts(items_fts, rowid, part, name, description) VALUES ('delete', old.rowid, old.part, old.name, old.description);
END;
CREATE TABLE IF NOT EXISTS imports(file TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

const COLUMNS: &str = "vendor, part, name, description, category_json, url, step, cad_pdf, image";

pub struct CatalogDb {
    conn: Connection,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub imported: Vec<String>,
    pub removed: Vec<String>,
    /// (file name, error text)
    pub skipped: Vec<(String, String)>,
}

fn book_item(r: &Row) -> rusqlite::Result<BookItem> {
    Ok(BookItem {
        vendor: r.get(0)?,
        part: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        category: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        url: r.get(5)?,
        step: r.get(6)?,
        cad_pdf: r.get(7)?,
        image: r.get(8)?,
    })
}

impl CatalogDb {
    pub fn open(path: &Path) -> Result<CatalogDb, rusqlite::Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let mut conn = Connection::open(path)?;
        let version: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key='schema'", [], |r| r.get(0)).optional().unwrap_or(None);
        if version.is_some_and(|v| v != SCHEMA_VERSION.to_string()) {
            drop(conn);
            for suffix in ["", "-wal", "-shm"] {
                let mut p = path.as_os_str().to_owned();
                p.push(suffix);
                std::fs::remove_file(p).ok();
            }
            conn = Connection::open(path)?;
        }
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<CatalogDb, rusqlite::Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<CatalogDb, rusqlite::Error> {
        conn.execute_batch(SCHEMA)?;
        conn.execute("INSERT OR IGNORE INTO meta(key, value) VALUES('schema', ?)", [SCHEMA_VERSION.to_string()])?;
        Ok(CatalogDb { conn })
    }

    /// Imports changed `*.json` files in `dir` and drops the rows of files that are gone.
    /// A file that cannot be read or parsed keeps its previous rows.
    pub fn sync_json_dir(&mut self, dir: &Path) -> SyncReport {
        let mut report = SyncReport::default();
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
            .filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path())))
            .collect();
        files.sort();
        for name in self.query_strings("SELECT file FROM imports ORDER BY file") {
            if !files.iter().any(|(n, _)| *n == name) {
                match self.remove_file(&name) {
                    Ok(()) => report.removed.push(name),
                    Err(e) => report.skipped.push((name, e.to_string())),
                }
            }
        }
        for (name, path) in &files {
            if let Err(error) = self.import_file(name, path, &mut report.imported) {
                report.skipped.push((name.clone(), error));
            }
        }
        report
    }

    fn query_strings(&self, sql: &str) -> Vec<String> {
        let Ok(mut stmt) = self.conn.prepare(sql) else { return Vec::new() };
        stmt.query_map([], |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }

    fn remove_file(&mut self, name: &str) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM items WHERE source = ?", [format!("{JSON_PREFIX}{name}")])?;
        tx.execute("DELETE FROM imports WHERE file = ?", [name])?;
        Self::recompute_winners_in(&tx)?;
        tx.commit()
    }

    fn import_file(&mut self, name: &str, path: &Path, imported: &mut Vec<String>) -> Result<(), String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        let size = meta.len() as i64;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as i64);
        let stored: Option<(i64, i64)> = self
            .conn
            .query_row("SELECT size, mtime FROM imports WHERE file = ?", [name], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        if stored == Some((size, mtime)) {
            return Ok(());
        }
        let items = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| parse_book(&t).map_err(|e| e.to_string()))?;
        let source = format!("{JSON_PREFIX}{name}");
        let write = |conn: &mut Connection| -> rusqlite::Result<()> {
            let tx = conn.transaction()?;
            tx.execute("DELETE FROM items WHERE source = ?", [&source])?;
            {
                // OR IGNORE: the first duplicate within a file wins, as `load_books` did.
                let mut ins = tx.prepare(
                    "INSERT OR IGNORE INTO items(vendor, part, name, description, category_json, cat1, cat2, url, step, cad_pdf, image, source)
                     VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
                )?;
                for i in &items {
                    let cat = |n: usize| i.category.get(n).map_or("", String::as_str);
                    ins.execute(params![
                        i.vendor,
                        i.part,
                        i.name,
                        i.description,
                        serde_json::to_string(&i.category).unwrap_or_else(|_| "[]".into()),
                        cat(0),
                        cat(1),
                        i.url,
                        i.step,
                        i.cad_pdf,
                        i.image,
                        source
                    ])?;
                }
            }
            tx.execute("INSERT OR REPLACE INTO imports(file, size, mtime) VALUES(?,?,?)", params![name, size, mtime])?;
            Self::recompute_winners_in(&tx)?;
            tx.commit()
        };
        write(&mut self.conn).map_err(|e| e.to_string())?;
        imported.push(name.to_owned());
        Ok(())
    }

    /// Sets `items.winner` to 1 for the row shown per (vendor, part): JSON beats any other source,
    /// and among JSON files the first file name wins. Other sources (the updater) tie-break by name.
    /// Takes a connection so callers can run it inside their own transaction (a `Transaction` derefs
    /// to one); a failure then rolls the whole change back instead of hiding rows.
    pub(crate) fn recompute_winners_in(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(&format!(
            "UPDATE items SET winner = 0 WHERE winner <> 0;
             UPDATE items SET winner = 1 WHERE rowid IN (
                 SELECT rowid FROM (SELECT rowid, ROW_NUMBER() OVER (
                     PARTITION BY vendor, part ORDER BY (source NOT LIKE '{JSON_PREFIX}%'), source) AS rn FROM items)
                 WHERE rn = 1);"
        ))
    }

    pub(crate) fn recompute_winners(&self) -> rusqlite::Result<()> {
        Self::recompute_winners_in(&self.conn)
    }

    /// Rows (at most `limit`, ordered by part) and the total match count. Text of 3+ characters
    /// uses the trigram index, shorter text a literal LIKE; neither treats the query as syntax.
    pub fn search(&self, query: &str, cat2: Option<&str>, limit: usize) -> (Vec<BookItem>, usize) {
        let q = query.trim();
        let mut conds = vec!["winner = 1".to_owned()];
        let mut args: Vec<String> = Vec::new();
        match q.chars().count() {
            0 => {}
            1..=2 => {
                let esc = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
                conds.push("(part||char(10)||name||char(10)||description) LIKE ? ESCAPE '\\'".to_owned());
                args.push(format!("%{esc}%"));
            }
            _ => {
                conds.push("rowid IN (SELECT rowid FROM items_fts WHERE items_fts MATCH ?)".to_owned());
                args.push(format!("\"{}\"", q.replace('"', "\"\"")));
            }
        }
        if let Some(c) = cat2 {
            conds.push("cat2 = ?".to_owned());
            args.push(c.to_owned());
        }
        let wh = conds.join(" AND ");
        let total = self
            .conn
            .query_row(&format!("SELECT count(*) FROM items WHERE {wh}"), params_from_iter(&args), |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize;
        let rows = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM items WHERE {wh} ORDER BY part COLLATE NOCASE LIMIT {limit}"))
            .and_then(|mut s| s.query_map(params_from_iter(&args), book_item)?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default();
        (rows, total)
    }

    pub fn categories(&self) -> Vec<String> {
        self.query_strings("SELECT DISTINCT cat2 FROM items WHERE winner = 1 AND cat2 <> '' ORDER BY cat2")
    }

    pub fn find(&self, part: &str) -> Option<BookItem> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM items WHERE winner = 1 AND part = ? LIMIT 1"), [part], book_item)
            .ok()
    }

    pub fn is_empty(&self) -> bool {
        self.conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM items)", [], |r| r.get(0)).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = include_str!("testdata/catalog-example.json");

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("labsetup-db-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn book(items: &[(&str, &str)]) -> String {
        let items: Vec<String> = items
            .iter()
            .map(|(p, n)| {
                format!(
                    r#"{{"part":{},"name":{},"category":["A","B"],"url":"https://example.com/x"}}"#,
                    serde_json::to_string(p).unwrap(),
                    serde_json::to_string(n).unwrap()
                )
            })
            .collect();
        format!(r#"{{"vendor":"Example Optics","items":[{}]}}"#, items.join(","))
    }

    fn synced(tag: &str, files: &[(&str, &str)]) -> (std::path::PathBuf, CatalogDb) {
        let dir = tmp(tag);
        for (n, t) in files {
            std::fs::write(dir.join(n), t).unwrap();
        }
        let mut db = CatalogDb::open_in_memory().unwrap();
        db.sync_json_dir(&dir);
        (dir, db)
    }

    #[test]
    fn imports_example_and_searches() {
        let dir = tmp("example");
        std::fs::write(dir.join("catalog-example.json"), EXAMPLE).unwrap();
        let mut db = CatalogDb::open_in_memory().unwrap();
        assert!(db.is_empty());
        let report = db.sync_json_dir(&dir);
        assert_eq!(report.imported, ["catalog-example.json"]);
        assert_eq!(db.search("ex-km", None, 500).0[0].part, "EX-KM1");
        assert_eq!(db.search("", Some("Posts"), 500).1, 1);
        assert_eq!(db.categories(), ["Lenses", "Mirror Mounts", "Posts"]);
        assert_eq!(db.find("EX-L100").unwrap().image.as_deref(), Some("https://example.com/img/EX-L100.png"));
    }

    #[test]
    fn unchanged_file_is_not_reimported() {
        let (dir, mut db) = synced("unchanged", &[("a.json", EXAMPLE)]);
        let report = db.sync_json_dir(&dir);
        assert!(report.imported.is_empty() && report.removed.is_empty() && report.skipped.is_empty());
    }

    #[test]
    fn changed_and_removed_files() {
        let (dir, mut db) = synced("changed", &[("a.json", EXAMPLE)]);
        std::fs::write(dir.join("a.json"), book(&[("EX-ONLY", "Only")])).unwrap();
        assert_eq!(db.sync_json_dir(&dir).imported, ["a.json"]);
        assert_eq!(db.search("", None, 500).1, 1);
        std::fs::remove_file(dir.join("a.json")).unwrap();
        assert_eq!(db.sync_json_dir(&dir).removed, ["a.json"]);
        assert!(db.is_empty());
    }

    #[test]
    fn malformed_file_keeps_previous_rows() {
        let (dir, mut db) = synced("malformed", &[("a.json", EXAMPLE)]);
        std::fs::write(dir.join("a.json"), "{").unwrap();
        let report = db.sync_json_dir(&dir);
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].0, "a.json");
        assert_eq!(db.search("ex-km", None, 500).0[0].part, "EX-KM1");
    }

    #[test]
    fn first_file_name_wins() {
        let (_d, db) = synced(
            "first",
            &[("b.json", &book(&[("EX-KM1", "From B")])), ("a.json", &book(&[("EX-KM1", "From A")]))],
        );
        let (rows, total) = db.search("ex-km1", None, 500);
        assert_eq!((total, rows[0].name.as_str()), (1, "From A"));
    }

    #[test]
    fn json_beats_other_sources() {
        let (_d, db) = synced("json-wins", &[("z.json", &book(&[("EX-KM1", "From JSON")]))]);
        db.conn
            .execute(
                "INSERT INTO items(vendor, part, name, description, category_json, cat1, cat2, url, source)
                 VALUES('Example Optics','EX-KM1','From update','','[]','','','u','update:thorlabs')",
                [],
            )
            .unwrap();
        db.recompute_winners().unwrap();
        assert_eq!(db.find("EX-KM1").unwrap().name, "From JSON");
        assert_eq!(db.search("", None, 500).1, 1);
    }

    #[test]
    fn every_part_has_a_winner_after_sync() {
        let (dir, mut db) = synced("winners", &[("a.json", EXAMPLE), ("b.json", EXAMPLE)]);
        let orphans = |db: &CatalogDb| -> i64 {
            db.conn
                .query_row(
                    "SELECT count(*) FROM (SELECT 1 FROM items GROUP BY vendor, part HAVING max(winner) = 0)",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(orphans(&db), 0);
        std::fs::remove_file(dir.join("a.json")).unwrap();
        db.sync_json_dir(&dir);
        assert_eq!((orphans(&db), db.search("", None, 500).1), (0, 3));
    }

    #[test]
    fn search_treats_query_as_literal() {
        let (_d, db) = synced("literal", &[("a.json", &book(&[("EX-A", "Mount for Ø1\" Optics"), ("TR50/M", "Rod")]))]);
        for q in ["Ø1\"", "tr50/m", "(", "*", "-"] {
            db.search(q, None, 500);
        }
        assert_eq!(db.search("ø1\"", None, 500).1, 1);
        assert_eq!(db.search("TR50/M", None, 500).1, 1);
        assert_eq!(db.search("Ø1", None, 500).1, 1);
    }

    #[test]
    fn short_queries_use_like_literally() {
        let (_d, db) = synced("short", &[("a.json", &book(&[("EX-A", "Mount"), ("EX-B", "Rod"), ("EX-C", "Mirror")]))]);
        assert_eq!(db.search("m", None, 500).1, 2);
        assert_eq!(db.search("%", None, 500).1, 0);
        assert_eq!(db.search("_", None, 500).1, 0);
    }

    #[test]
    fn schema_version_mismatch_rebuilds() {
        let dir = tmp("schema");
        let path = dir.join("catalog.sqlite");
        let mut db = CatalogDb::open(&path).unwrap();
        std::fs::write(dir.join("a.json"), EXAMPLE).unwrap();
        db.sync_json_dir(&dir);
        assert!(!db.is_empty());
        db.conn.execute("UPDATE meta SET value='0' WHERE key='schema'", []).unwrap();
        drop(db);
        assert!(CatalogDb::open(&path).unwrap().is_empty());
    }
}
