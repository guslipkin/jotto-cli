#!/usr/bin/env python3
"""Build data/flights.tsv, the airports and routes `flights` embeds.

Run by hand when the routes should be refreshed, not from build.rs: it reads
thousands of Wikipedia articles, far too slow (and too unkind to Wikipedia) for
every build, while the result is about 400 KB and is committed.

    python3 scripts/build-flights-data.py --cache-dir ~/.cache/flights-wiki

No free, current database of airline routes exists (OpenFlights' stopped in
2014), so routes come from the "Airlines and destinations" section of each
airport's English Wikipedia article (CC BY-SA 4.0), which editors keep current
and which marks seasonal and charter service. Airports, their IATA codes and
their coordinates come from OurAirports' airports.csv (public domain,
https://ourairports.com/data/), whose `wikipedia_link` column ties each
article to its code; a destination link is resolved through Wikipedia's
redirects to an article in that map, and links that don't reach one (a city
rather than its airport, an airport with no IATA code) are dropped and counted.

Counted routes are scheduled passenger flights, year-round or seasonal; cargo
never is. Charter flights are left out unless `--include-charter`. A route
counts when either airport's article lists it, since articles are edited
independently and one side is often more complete.

A move in the game (a "hop") joins two codes one letter apart, but direct
flights between such codes are rare: about 340 of the ~34,000 routes, in
clusters of at most a couple of dozen airports, which make only a few dozen
puzzles. So a hop is any ticket with connections, and its length is the
shortest such itinerary's, every connection counted. The best itinerary is
written for each limit on connections up to MAX_CONNECTIONS, along with the
airports they touch.

The script reports the hops by number of connections and the network's largest
components, and counts the start/end pairs that make a valid puzzle (codes
differ in all three letters, and at least two routes join them) by the number of
hops in their shortest route, which decides the difficulty levels they're dealt
at. The game reads those counts for its help rather than counting every pair
itself, which would take seconds.
Progress goes to stdout and to `--log`.

File format (tab-separated text, `#` comments):
    A<TAB>CODE<TAB>latitude<TAB>longitude<TAB>city<TAB>country (ISO code)
    H<TAB>CODE<TAB>CODE<TAB>c0<TAB>c1<TAB>c2
        the best itinerary with at most 0, 1 and 2 connections: comma-separated
        codes in flying order from the first code, "-" if direct, "x" if there's
        no ticket within that limit
    P<TAB>connections<TAB>hops<TAB>count
        valid puzzles, with at most that many connections per hop, whose
        shortest route has that many hops
"""

import argparse
import csv
import heapq
import io
import json
import math
import re
import sys
import time
import urllib.parse
import urllib.request
from collections import defaultdict, deque
from datetime import datetime, timezone
from pathlib import Path

AIRPORTS_URL = "https://davidmegginson.github.io/ourairports-data/airports.csv"
API_URL = "https://en.wikipedia.org/w/api.php"
USER_AGENT = (
    "jotto-cli flights data builder "
    "(https://github.com/guslipkin/jotto-cli; run by hand, a few times a year)"
)
# Wikipedia's API takes up to 50 titles a request; content requests may come
# back partial and continue.
BATCH = 50
# Seconds between requests. Wikimedia asks bots to make requests one at a time;
# a short pause on top keeps the whole run (~200 requests) polite.
REQUEST_PAUSE = 0.5

# Most connections a hop between codes one letter apart may take: a ticket with
# up to this many stops. The game's standard levels use one; custom games may
# choose none (too few puzzles to deal from, but playable), one or two. More
# stops let the shortest itinerary wander through four or five airports no one
# would book.
MAX_CONNECTIONS = 2
EARTH_RADIUS_NM = 3_440.065

# Subsections of "Airlines and destinations" that aren't passenger service.
NON_PASSENGER = re.compile(r"cargo|freight|mail", re.I)
# Inline labels in a destinations cell. Everything after one, up to the next
# label or the end of the airline's row, has that kind of service.
LABEL = re.compile(
    r"'''\s*(seasonal\s+charter|charter|seasonal)\s*:?\s*'''\s*:?|"
    r"\b(seasonal\s+charter|charter|seasonal)\s*:",
    re.I,
)
LINK = re.compile(r"\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|[^\]]*)?\]\]")

log_file = None


def log(msg):
    print(msg, flush=True)
    if log_file:
        log_file.write(msg + "\n")
        log_file.flush()


def norm_title(title):
    """Wikipedia's canonical form: spaces not underscores, first letter upper."""
    title = urllib.parse.unquote(title).replace("_", " ").strip()
    title = re.sub(r"\s+", " ", title)
    return title[:1].upper() + title[1:]


def load_airports(path):
    """IATA code -> (lat, lon, wikipedia title or None) for open airports."""
    if path:
        text = Path(path).read_text(encoding="utf-8")
    else:
        log(f"Downloading {AIRPORTS_URL}")
        req = urllib.request.Request(AIRPORTS_URL, headers={"User-Agent": USER_AGENT})
        text = urllib.request.urlopen(req).read().decode("utf-8")
    airports = {}
    scheduled = set()
    for row in csv.DictReader(io.StringIO(text)):
        code = row["iata_code"].strip().upper()
        if not re.fullmatch(r"[A-Z]{3}", code) or row["type"] == "closed":
            continue
        link = row["wikipedia_link"].strip()
        title = None
        if "/wiki/" in link and "en.wikipedia.org" in link:
            title = norm_title(link.split("/wiki/", 1)[1])
        # A code can be listed twice (an old field and its replacement); keep
        # the one with scheduled service.
        if code in airports and code in scheduled:
            continue
        city = " ".join(row["municipality"].split())
        airports[code] = (
            float(row["latitude_deg"]),
            float(row["longitude_deg"]),
            title,
            city,
            row["iso_country"].strip(),
        )
        if row["scheduled_service"] == "yes":
            scheduled.add(code)
    return airports, scheduled


class Wiki:
    """Batched, cached reads of article text and redirect targets."""

    def __init__(self, cache_dir):
        self.cache_dir = Path(cache_dir)
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.pages_path = self.cache_dir / "pages.json"
        self.redirects_path = self.cache_dir / "redirects.json"
        self.pages = self._load(self.pages_path)
        self.redirects = self._load(self.redirects_path)
        self.last_request = 0.0

    @staticmethod
    def _load(path):
        if path.exists():
            return json.loads(path.read_text(encoding="utf-8"))
        return {}

    def _save(self):
        self.pages_path.write_text(json.dumps(self.pages), encoding="utf-8")
        self.redirects_path.write_text(json.dumps(self.redirects), encoding="utf-8")

    def _get(self, params):
        wait = REQUEST_PAUSE - (time.monotonic() - self.last_request)
        if wait > 0:
            time.sleep(wait)
        params = {"format": "json", "formatversion": "2", **params}
        url = API_URL + "?" + urllib.parse.urlencode(params)
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        for attempt in range(5):
            try:
                with urllib.request.urlopen(req, timeout=60) as resp:
                    self.last_request = time.monotonic()
                    return json.load(resp)
            except Exception as err:  # network hiccups and 429s: back off and retry
                log(f"  request failed ({err}); retrying")
                time.sleep(5 * (attempt + 1))
        raise SystemExit("Wikipedia API kept failing; rerun to continue from the cache")

    def _query(self, titles, content):
        """Runs one batch, following `continue`; records redirects and content."""
        params = {"action": "query", "redirects": "1", "titles": "|".join(titles)}
        if content:
            params.update(prop="revisions", rvprop="content", rvslots="main")
        cont = {}
        while True:
            data = self._get({**params, **cont})
            query = data.get("query", {})
            hops = {}
            for r in query.get("normalized", []) + query.get("redirects", []):
                hops[r["from"]] = r["to"]
            for t in titles:
                final, seen = t, set()
                while final in hops and final not in seen:
                    seen.add(final)
                    final = hops[final]
                self.redirects[t] = final
            for page in query.get("pages", []):
                if content and "revisions" in page:
                    self.pages[page["title"]] = page["revisions"][0]["slots"]["main"]["content"]
                elif page.get("missing"):
                    self.pages.setdefault(page["title"], "")
            if "continue" not in data:
                return
            cont = data["continue"]

    def fetch(self, titles, content):
        todo = [t for t in dict.fromkeys(titles) if t not in self.redirects]
        if content:
            todo = [
                t
                for t in dict.fromkeys(titles)
                if t not in self.redirects or self.redirects[t] not in self.pages
            ]
        for i in range(0, len(todo), BATCH):
            self._query(todo[i : i + BATCH], content)
            log(f"  {min(i + BATCH, len(todo))}/{len(todo)} titles")
            if (i // BATCH) % 10 == 9:
                self._save()
        self._save()

    def resolve(self, title):
        return self.redirects.get(title, title)

    def text(self, title):
        return self.pages.get(self.resolve(title), "")


def passenger_section(text):
    """Text of "Airlines and destinations", less its cargo subsections."""
    m = re.search(r"^==\s*Airlines\s+and\s+destinations\s*==\s*$", text, re.M | re.I)
    if not m:
        return ""
    rest = text[m.end() :]
    end = re.search(r"^==[^=].*?[^=]==\s*$", rest, re.M)
    section = rest[: end.start()] if end else rest
    # Split on subsection headings; drop the non-passenger ones.
    parts = re.split(r"^(===+[^=\n]+===+)\s*$", section, flags=re.M)
    kept = [parts[0]]
    for heading, body in zip(parts[1::2], parts[2::2]):
        if not NON_PASSENGER.search(heading):
            kept.append(body)
    return "\n".join(kept)


def strip_noise(text):
    text = re.sub(r"<!--.*?-->", "\n", text, flags=re.S)
    text = re.sub(r"<ref[^>/]*/>", "", text)
    text = re.sub(r"<ref[^>]*>.*?</ref>", "", text, flags=re.S | re.I)
    # File/image links carry captions with their own links.
    text = re.sub(r"\[\[(?:File|Image):[^\[\]]*(?:\[\[[^\]]*\]\][^\[\]]*)*\]\]", "", text)
    return text


def destination_links(section):
    """(link title, kind) for each destination link; kind is "", "s" or "c"."""
    out = []
    kind = ""
    for line in strip_noise(section).split("\n"):
        stripped = line.strip()
        # A new airline row (template "| [[Airline]] |" or table "|-") resets
        # the label: a label only covers the rest of its own airline's row.
        if stripped.startswith("|-") or re.match(r"^\|\s*\[\[", stripped) or stripped.startswith("{{"):
            kind = ""
        pos = 0
        while True:
            label = LABEL.search(line, pos)
            stop = label.start() if label else len(line)
            for link in LINK.finditer(line, pos, stop):
                out.append((link.group(1), kind))
            if not label:
                break
            word = (label.group(1) or label.group(2)).lower()
            kind = "c" if "charter" in word else "s"
            pos = label.end()
    return out


def bridges(adj):
    """Set of frozenset edges that are bridges (iterative Tarjan)."""
    disc, low, out = {}, {}, set()
    timer = 0
    for root in adj:
        if root in disc:
            continue
        disc[root] = low[root] = timer
        timer += 1
        stack = [(root, None, iter(adj[root]))]
        while stack:
            node, parent, it = stack[-1]
            advanced = False
            for nxt in it:
                if nxt == parent:
                    continue
                if nxt in disc:
                    low[node] = min(low[node], disc[nxt])
                else:
                    disc[nxt] = low[nxt] = timer
                    timer += 1
                    stack.append((nxt, node, iter(adj[nxt])))
                    advanced = True
                    break
            if not advanced:
                stack.pop()
                if parent is not None:
                    low[parent] = min(low[parent], low[node])
                    if low[node] > disc[parent]:
                        out.add(frozenset((parent, node)))
    return out


def puzzles_by_hops(adj, hop_nm):
    """Valid puzzles (ordered start/end pairs) by the number of hops in their
    shortest-distance route, which sets the difficulty levels they're dealt at.

    Runs Dijkstra from every airport, as the game does for each puzzle it deals,
    with the same distances (`hop_nm`, from the coordinates as written). Two
    routes join a pair exactly when its shortest route crosses a hop that isn't
    a bridge (one on a loop), which the game checks the same way."""
    br = bridges(adj)
    counts = defaultdict(int)
    for n, s in enumerate(sorted(adj)):
        if n % 500 == 0:
            log(f"  counting puzzles from {n}/{len(adj)} airports")
        dist = {s: 0.0}
        hops = {s: 0}
        two = {s: False}
        done = set()
        heap = [(0.0, s)]
        while heap:
            d, node = heapq.heappop(heap)
            if node in done:
                continue
            done.add(node)
            for nxt in adj[node]:
                nd = d + hop_nm[(node, nxt) if node < nxt else (nxt, node)]
                if nd < dist.get(nxt, math.inf):
                    dist[nxt] = nd
                    hops[nxt] = hops[node] + 1
                    two[nxt] = two[node] or frozenset((node, nxt)) not in br
                    heapq.heappush(heap, (nd, nxt))
        for t in done:
            if two[t] and all(x != y for x, y in zip(s, t)):
                counts[hops[t]] += 1
    return dict(sorted(counts.items()))


def components(adj):
    seen, sizes = set(), []
    for s in adj:
        if s in seen:
            continue
        seen.add(s)
        queue, n = deque([s]), 0
        while queue:
            node = queue.popleft()
            n += 1
            for nxt in adj[node]:
                if nxt not in seen:
                    seen.add(nxt)
                    queue.append(nxt)
        sizes.append(n)
    return sorted(sizes, reverse=True)


def great_circle_nm(a, b):
    """Great-circle distance in nautical miles between (lat, lon) points; the game
    computes the same from the written coordinates."""
    lat1, lon1, lat2, lon2 = map(math.radians, (a[0], a[1], b[0], b[1]))
    h = math.sin((lat2 - lat1) / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(
        (lon2 - lon1) / 2
    ) ** 2
    return 2 * EARTH_RADIUS_NM * math.asin(min(1.0, math.sqrt(h)))


def find_hops(flights, coords):
    """The shortest itinerary between each pair of airports one letter apart for
    each limit on connections, 0 to MAX_CONNECTIONS, as {(a, b): [itinerary or
    None per limit]} with a < b. An itinerary is its connections, in flying order.
    The game lets a custom level choose the limit, and the best ticket within one
    limit isn't the best within another less a stop, so each is kept.

    Built in two halves rather than by a general search, since the bound is small:
    from each airport, the best way (direct or one stop) to everywhere, then each
    target is reached either that way or with one more flight in from a neighbour
    of the target. Three flights is enough here (MAX_CONNECTIONS is 2)."""
    assert MAX_CONNECTIONS == 2
    dist = {}

    def leg(x, y):
        key = (x, y) if x < y else (y, x)
        if key not in dist:
            dist[key] = great_circle_nm(coords[x], coords[y])
        return dist[key]

    by_pattern = defaultdict(list)
    for c in flights:
        for i in range(3):
            by_pattern[c[:i] + "_" + c[i + 1 :]].append(c)
    targets = defaultdict(set)
    for group in by_pattern.values():
        for c in group:
            targets[c].update(t for t in group if t > c)

    hops = {}
    for n, a in enumerate(sorted(flights)):
        if n % 500 == 0:
            log(f"  itineraries from {n}/{len(flights)} airports")
        if not targets[a]:
            continue
        # best[y] = (distance, connection or None) for one or two flights a..y.
        best = {y: (leg(a, y), None) for y in flights[a]}
        for x in flights[a]:
            d1 = leg(a, x)
            for y in flights[x]:
                if y == a:
                    continue
                d = d1 + leg(x, y)
                if y not in best or d < best[y][0]:
                    best[y] = (d, x)
        for t in targets[a]:
            direct = [] if t in flights[a] else None
            one = None
            options = []
            if t in best:
                d, x = best[t]
                one = [x] if x else []
                options.append((d, one))
            for y in flights[t]:
                if y in best and y != a:
                    d, x = best[y]
                    if x != t:
                        options.append((d + leg(y, t), ([x] if x else []) + [y]))
            if options:
                hops[(a, t)] = [direct, one, min(options)[1]]
    return hops


def main():
    global log_file
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--airports", help="OurAirports airports.csv (downloaded if omitted)")
    p.add_argument("--cache-dir", default=".flights-cache", help="where fetched articles are kept")
    p.add_argument("--out", default="data/flights.tsv")
    p.add_argument("--log", default="flights-build.log")
    p.add_argument("--include-charter", action="store_true", help="count charter-only routes")
    args = p.parse_args()
    log_file = open(args.log, "w", encoding="utf-8")

    airports, scheduled = load_airports(args.airports)
    log(f"{len(airports)} airports with IATA codes, {len(scheduled)} with scheduled service")

    wiki = Wiki(args.cache_dir)
    sources = {c: airports[c][2] for c in scheduled if airports[c][2]}
    log(f"Fetching {len(sources)} airport articles")
    wiki.fetch(list(sources.values()), content=True)

    # Article title (after redirects) -> code, for every airport with an
    # article. The OurAirports links themselves may be redirects.
    title_codes = defaultdict(set)
    linked = [(c, a[2]) for c, a in airports.items() if a[2]]
    log(f"Resolving {len(linked)} OurAirports article links")
    wiki.fetch([t for _, t in linked], content=False)
    for code, title in linked:
        title_codes[wiki.resolve(title)].add(code)
    title_code = {}
    for title, codes in title_codes.items():
        # Two codes on one article (an airport's old and new code, or a closed
        # field it replaced): take the one with scheduled service, if only one has.
        live = codes & scheduled or codes
        title_code[title] = next(iter(live)) if len(live) == 1 else None

    pages = {code: passenger_section(wiki.text(title)) for code, title in sources.items()}
    raw = {code: destination_links(sec) for code, sec in pages.items()}
    log(f"{sum(1 for s in pages.values() if s)} articles have a destinations section")

    targets = sorted({norm_title(t) for links in raw.values() for t, _ in links})
    log(f"Resolving {len(targets)} destination links")
    wiki.fetch(targets, content=False)

    flights = defaultdict(set)
    unresolved = defaultdict(int)
    for code, links in raw.items():
        for title, kind in links:
            dest = title_code.get(wiki.resolve(norm_title(title)))
            if dest is None:
                unresolved[norm_title(title)] += 1
                continue
            if dest != code and (kind != "c" or args.include_charter):
                flights[code].add(dest)
                flights[dest].add(code)
    log(
        f"{sum(map(len, flights.values())) // 2} routes between {len(flights)} airports; "
        f"{len(unresolved)} link targets didn't resolve to an airport"
    )
    for title, n in sorted(unresolved.items(), key=lambda x: -x[1])[:40]:
        log(f"  unresolved: {title} ({n})")

    hops = find_hops(flights, {c: airports[c][:2] for c in flights})
    used = {c for (a, b), its in hops.items() for it in its if it is not None for c in (a, b, *it)}
    # Distances from the coordinates as written, so they match the game's.
    written = {c: (round(airports[c][0], 4), round(airports[c][1], 4)) for c in used}
    by_hops = {}
    for limit in range(MAX_CONNECTIONS + 1):
        adj = defaultdict(list)
        hop_nm = {}
        for (a, b), its in hops.items():
            via = its[limit]
            if via is None:
                continue
            adj[a].append(b)
            adj[b].append(a)
            stops = [a, *via, b]
            hop_nm[(a, b)] = sum(
                great_circle_nm(written[x], written[y]) for x, y in zip(stops, stops[1:])
            )
        for node in adj:
            adj[node].sort()
        log(
            f"Up to {limit} connections: {len(hop_nm)} hops between {len(adj)} airports; "
            f"largest components {components(adj)[:6]}"
        )
        by_hops[limit] = puzzles_by_hops(adj, hop_nm)
        log(f"  valid puzzles by shortest route's hops: {by_hops[limit]}")
        log(f"  valid puzzles in all: {sum(by_hops[limit].values()):,}")
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(f"# flights data, built {stamp} by scripts/build-flights-data.py\n")
        f.write("# Routes: English Wikipedia airport articles, CC BY-SA 4.0\n")
        f.write("# Airports and coordinates: OurAirports (https://ourairports.com/data/), public domain\n")
        f.write(f"# Charter-only routes {'included' if args.include_charter else 'excluded'}\n")
        for code in sorted(used):
            lat, lon, _, city, country = airports[code]
            f.write(f"A\t{code}\t{lat:.4f}\t{lon:.4f}\t{city}\t{country}\n")
        for (a, b), its in sorted(hops.items()):
            cols = ["x" if it is None else ",".join(it) or "-" for it in its]
            f.write("\t".join(["H", a, b, *cols]) + "\n")
        for limit, counts in by_hops.items():
            for n, count in counts.items():
                f.write(f"P\t{limit}\t{n}\t{count}\n")
    log(f"Wrote {args.out}")

if __name__ == "__main__":
    sys.exit(main())
