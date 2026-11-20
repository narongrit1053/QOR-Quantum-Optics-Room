"""Build a personal Thorlabs catalog (docs/CATALOG-FORMAT.md) from the thorlabs.com store search.

    python scripts/catalog/fetch-thorlabs.py                      # the default sections below
    python scripts/catalog/fetch-thorlabs.py --section "Fiber"    # one or more sections of your own

Each request returns 200 items with breadcrumb, title and CAD links, one request every 30 s
(thorlabs.com robots.txt Crawl-delay). Raw pages are cached in catalog/.cache, so an interrupted run
resumes where it stopped. The search caps at 10,000 hits; for a bigger section every second-level
category is searched too. Writes catalog/thorlabs.json. For your own use only: see catalog/README.md.
"""
import argparse, html, json, pathlib, re, time, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2] / "catalog"
SECTIONS = ["Optomechanical Components", "Fiber", "Optical Elements"]
EXCLUDE = ["Enclosure Systems", "Instrumentation Shelves", "Optomechanics Kits", "Optomechanics Capabilities"]
PAGE, CAP, DELAY = 200, 10000, 30
QUERY = """query($q:String,$n:Int,$a:String){ products(storeId:"Thorlabs-Website", cultureName:"en-US",
 query:$q, first:$n, after:$a){ totalCount items { code descriptions { content reviewType } assets { name url group } } } }"""
_last = [0.0]


def gql(variables):
    time.sleep(max(0, DELAY - (time.time() - _last[0])))
    req = urllib.request.Request("https://www.thorlabs.com/graphql",
                                 data=json.dumps({"query": QUERY, "variables": variables}).encode(),
                                 headers={"Content-Type": "application/json", "User-Agent": "Mozilla/5.0 (personal catalog; 1 req/30s)"})
    for attempt in range(3):
        try:
            data = json.loads(urllib.request.urlopen(req, timeout=120).read())
            _last[0] = time.time()
            if data.get("data") and data["data"].get("products"):
                return data["data"]["products"]
            print("  no data:", str(data.get("errors"))[:300])
        except Exception as e:
            print("  retry", attempt + 1, e)
        _last[0] = time.time() + 30  # back off before the retry
    raise SystemExit(f"request failed 3 times: {variables}")


def page(cache, q, off):
    f = cache / (re.sub(r"[^a-z0-9]+", "-", q.lower()) + f"-{off:06d}.json")
    if not f.exists():
        f.write_text(json.dumps(gql({"q": q, "n": PAGE, "a": str(off)})), "utf-8")
    return json.loads(f.read_text("utf-8"))


def strip(s):
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<[^>]+>", " ", s or ""))).strip()


def to_item(it, section, exclude):
    """Catalog entry for one search hit, or None when it is outside `section` or in an excluded category."""
    d = {x["reviewType"]: x["content"] for x in it.get("descriptions") or [] if x.get("content")}
    if "Breadcrumb" not in d:
        return None
    crumbs = [c[0] for c in json.loads(d["Breadcrumb"])][:-2][::-1]  # leaf-first [family .. All Products, item] -> top-down
    if section not in crumbs or any(c in exclude for c in crumbs):
        return None
    assets = it.get("assets") or []
    step = next((a["url"] for a in assets if a.get("group") == "Step"), None) or \
        next((a["url"] for a in assets if re.search(r"\.(step|stp)$", a.get("name") or "", re.I)), None)
    pdf = next((a["url"] for a in assets if re.search("CAD|Drawing", a.get("group") or "")
                and (a.get("name") or "").lower().endswith(".pdf")), None)
    title = strip(d.get("Title"))
    e = {"part": it["code"], "name": title or it["code"],
         "description": strip(f"{d['FamilyPageName']}: {d.get('Title', '')}") if d.get("FamilyPageName") else title,
         "category": crumbs[crumbs.index(section):],
         "url": "https://www.thorlabs.com" + (d.get("ItemUrl") or "/item/" + it["code"])}
    if step:
        e["step"] = step
    if pdf:
        e["cad_pdf"] = pdf
    return e


def fetch(cache, section, exclude, items):
    queries, done, second = [section], set(), set()
    while queries:
        q = queries.pop(0)
        if q in done:
            continue
        done.add(q)
        off, total = 0, 1
        while off < min(total, CAP):
            p = page(cache, q, off)
            total = p["totalCount"]
            for it in p["items"]:
                e = to_item(it, section, exclude)
                if e:
                    if len(e["category"]) > 1:
                        second.add(e["category"][1])
                    items.setdefault(e["part"], e)
            off += PAGE
            print(f"{section} / {q}: {min(off, total)}/{total}, catalog has {len(items)} items", flush=True)
        if q == section and total > CAP:
            queries += sorted(second)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--section", action="append", help=f"breadcrumb name to fetch (default: {', '.join(SECTIONS)})")
    ap.add_argument("--exclude", action="append", help=f"category to leave out (default: {', '.join(EXCLUDE)})")
    ap.add_argument("--out", default=str(ROOT / "thorlabs.json"))
    a = ap.parse_args()
    cache = ROOT / ".cache"
    cache.mkdir(parents=True, exist_ok=True)
    items = {}
    for s in a.section or SECTIONS:
        fetch(cache, s, set(a.exclude or EXCLUDE), items)
    out = sorted(items.values(), key=lambda i: i["part"].lower())
    pathlib.Path(a.out).write_text(json.dumps({"vendor": "Thorlabs", "items": out}, ensure_ascii=False, indent=1), "utf-8")
    print(f"wrote {len(out)} items to {a.out}")


if __name__ == "__main__":
    main()
