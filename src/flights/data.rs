use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::sync::OnceLock;

use crate::dict::DAILY_EPOCH_DAY;

/// Airports and the hops between codes one letter apart, written by
/// `scripts/build-flights-data.py` (see its docstring for the format and sources).
/// Committed rather than built by `build.rs`, since building it reads thousands of
/// Wikipedia articles.
static DATA: &str = include_str!("../../data/flights.tsv");

/// Fixed seed for the daily puzzles, so every copy of the same build agrees on each day's.
const DAILY_SEED: u64 = 0x464C_4947_4854_5321; // "FLIGHTS!"

/// Most connections a hop may have in any game; `scripts/build-flights-data.py`
/// uses the same bound (and says why), and the data holds the best itinerary
/// within each limit up to it.
pub const MAX_CONNECTIONS: usize = 2;

/// Connections a hop may have in the standard levels (custom games choose).
/// Two made several times as many puzzles, but mostly by tickets few would book.
pub const STANDARD_CONNECTIONS: usize = 1;

/// Mean Earth radius in nautical miles, for great-circle distances.
const EARTH_RADIUS_NM: f64 = 3_440.065;

/// Random starts tried before giving up on finding a puzzle; any connected graph
/// with a cycle finds one in a handful.
const PUZZLE_ATTEMPTS: usize = 10_000;
/// Random ends tried from each start.
const END_ATTEMPTS: usize = 8;

/// One move: a ticket from an airport to one whose code is one letter different,
/// direct or with connections (the shortest itinerary of up to three flights).
#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    pub to: usize,
    /// Airports connected through, in flying order.
    pub via: Vec<usize>,
    /// Length in nautical miles, every flight of the itinerary counted.
    pub distance: f64,
}

/// The playable network: every airport with a hop to a code one letter away, the
/// airports those hops connect through, and the hops.
#[derive(Debug)]
pub struct Network {
    /// The most connections a hop here has.
    max_connections: usize,
    codes: Vec<String>,
    coords: Vec<(f64, f64)>,
    /// `(city, ISO country code)`, either possibly empty.
    places: Vec<(String, String)>,
    /// Hops from each airport, sorted by destination. Connection-only airports have none.
    adj: Vec<Vec<Hop>>,
    /// Hops that are bridges: the only way between the two sides of the network
    /// they join. Puzzles need a hop that leaves this set (see [`Network::has_two_routes`]).
    bridges: HashSet<(usize, usize)>,
    /// Valid puzzles by the number of hops in their shortest route, counted by
    /// `scripts/build-flights-data.py`, since counting every pair takes seconds.
    puzzle_counts: Vec<(usize, usize)>,
}

/// Returns the embedded network whose hops have at most `max_connections`
/// connections (capped at [`MAX_CONNECTIONS`]), parsed on first use.
pub fn network(max_connections: usize) -> &'static Network {
    static NETWORKS: [OnceLock<Network>; MAX_CONNECTIONS + 1] =
        [const { OnceLock::new() }; MAX_CONNECTIONS + 1];
    let limit = max_connections.min(MAX_CONNECTIONS);
    NETWORKS[limit]
        .get_or_init(|| Network::parse(DATA, limit).expect("data/flights.tsv is well formed"))
}

/// Great-circle distance in nautical miles between two `(lat, lon)` points (haversine).
pub fn great_circle_nm(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lon1) = (a.0.to_radians(), a.1.to_radians());
    let (lat2, lon2) = (b.0.to_radians(), b.1.to_radians());
    let h = ((lat2 - lat1) / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * ((lon2 - lon1) / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_NM * h.sqrt().min(1.0).asin()
}

/// Number of positions at which two codes differ.
pub fn letters_apart(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).filter(|(x, y)| x != y).count()
}

/// Dijkstra's queue entry, ordered so the heap pops the shortest distance first.
#[derive(PartialEq)]
struct Entry(f64, usize);

impl Eq for Entry {}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Network {
    /// Parses the data file's format, taking each hop's best itinerary with at most
    /// `max_connections` connections. Public so tests can build small networks.
    pub fn parse(text: &str, max_connections: usize) -> Result<Self, String> {
        if max_connections > MAX_CONNECTIONS {
            return Err(format!("hops have at most {MAX_CONNECTIONS} connections"));
        }
        let mut codes = Vec::new();
        let mut coords = Vec::new();
        let mut places = Vec::new();
        let mut routes = Vec::new();
        let mut puzzle_counts = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let bad = || format!("line {}: can't read '{line}'", n + 1);
            match fields.as_slice() {
                // City and country may be missing (and their tabs trimmed with them).
                ["A", code, lat, lon, place @ ..] if place.len() <= 2 => {
                    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
                        return Err(bad());
                    }
                    codes.push(code.to_string());
                    coords.push((
                        lat.parse().map_err(|_| bad())?,
                        lon.parse().map_err(|_| bad())?,
                    ));
                    let field = |i: usize| place.get(i).map_or("", |s| s.trim()).to_string();
                    places.push((field(0), field(1)));
                }
                ["H", a, b, itineraries @ ..] if itineraries.len() == MAX_CONNECTIONS + 1 => {
                    let via = itineraries[max_connections];
                    if via != "x" {
                        routes.push((a.to_string(), b.to_string(), via.to_string(), n + 1));
                    }
                }
                ["P", limit, hops, count] => {
                    if limit.parse::<usize>().map_err(|_| bad())? == max_connections {
                        puzzle_counts.push((
                            hops.parse().map_err(|_| bad())?,
                            count.parse().map_err(|_| bad())?,
                        ));
                    }
                }
                _ => return Err(bad()),
            }
        }
        let mut network = Self {
            max_connections,
            adj: vec![Vec::new(); codes.len()],
            codes,
            coords,
            places,
            bridges: HashSet::new(),
            puzzle_counts,
        };
        let mut order: Vec<usize> = (0..network.codes.len()).collect();
        order.sort_by(|&x, &y| network.codes[x].cmp(&network.codes[y]));
        // Codes are looked up by binary search, so they must be sorted and unique.
        if order.iter().enumerate().any(|(i, &x)| i != x)
            || network.codes.windows(2).any(|w| w[0] == w[1])
        {
            return Err("airports must be listed once each, sorted by code".to_string());
        }
        for (a, b, via, line) in routes {
            let unlisted = || format!("line {line}: hop through an unlisted airport");
            let (Some(x), Some(y)) = (network.index_of(&a), network.index_of(&b)) else {
                return Err(unlisted());
            };
            if letters_apart(&a, &b) != 1 {
                return Err(format!("line {line}: {a} and {b} aren't one letter apart"));
            }
            if via != "-" && via.split(',').count() > max_connections {
                return Err(format!("line {line}: too many connections"));
            }
            let via = if via == "-" {
                Vec::new()
            } else {
                via.split(',')
                    .map(|c| network.index_of(c).ok_or_else(unlisted))
                    .collect::<Result<Vec<_>, _>>()?
            };
            let stops: Vec<usize> = std::iter::once(x)
                .chain(via.iter().copied())
                .chain(std::iter::once(y))
                .collect();
            let distance = stops
                .windows(2)
                .map(|w| great_circle_nm(network.coords[w[0]], network.coords[w[1]]))
                .sum();
            let back: Vec<usize> = via.iter().rev().copied().collect();
            network.adj[x].push(Hop {
                to: y,
                via,
                distance,
            });
            network.adj[y].push(Hop {
                to: x,
                via: back,
                distance,
            });
        }
        for list in &mut network.adj {
            list.sort_by_key(|h| h.to);
            list.dedup_by_key(|h| h.to);
        }
        network.bridges = network.find_bridges();
        Ok(network)
    }

    /// Number of airports.
    pub fn len(&self) -> usize {
        self.codes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }

    /// Number of hops.
    pub fn hop_count(&self) -> usize {
        self.adj.iter().map(Vec::len).sum::<usize>() / 2
    }

    pub fn code(&self, idx: usize) -> &str {
        &self.codes[idx]
    }

    /// The most connections a hop here has.
    pub fn max_connections(&self) -> usize {
        self.max_connections
    }

    /// Where an airport is, as "City, CC" (or whichever of them is known).
    pub fn place(&self, idx: usize) -> String {
        let (city, country) = &self.places[idx];
        match (city.is_empty(), country.is_empty()) {
            (false, false) => format!("{city}, {country}"),
            (false, true) => city.clone(),
            _ => country.clone(),
        }
    }

    /// `(latitude, longitude)` in degrees.
    pub fn coords(&self, idx: usize) -> (f64, f64) {
        self.coords[idx]
    }

    /// Index of an upper-case code.
    pub fn index_of(&self, code: &str) -> Option<usize> {
        self.codes.binary_search_by(|c| c.as_str().cmp(code)).ok()
    }

    /// Hops from `idx`, sorted by destination.
    pub fn hops(&self, idx: usize) -> &[Hop] {
        &self.adj[idx]
    }

    /// The hop from `a` to `b`, if there is one.
    pub fn hop(&self, a: usize, b: usize) -> Option<&Hop> {
        self.adj[a]
            .binary_search_by_key(&b, |h| h.to)
            .ok()
            .map(|pos| &self.adj[a][pos])
    }

    /// The shortest route from `start` to `end` by total distance (Dijkstra), as its
    /// length in nautical miles and its airports, `start` and `end` included.
    pub fn shortest(&self, start: usize, end: usize) -> Option<(f64, Vec<usize>)> {
        let mut dist = vec![f64::INFINITY; self.len()];
        let mut prev = vec![usize::MAX; self.len()];
        let mut heap = BinaryHeap::new();
        dist[start] = 0.0;
        heap.push(Entry(0.0, start));
        while let Some(Entry(d, node)) = heap.pop() {
            if node == end {
                break;
            }
            if d > dist[node] {
                continue;
            }
            for &Hop {
                to: next, distance, ..
            } in &self.adj[node]
            {
                let nd = d + distance;
                if nd < dist[next] {
                    dist[next] = nd;
                    prev[next] = node;
                    heap.push(Entry(nd, next));
                }
            }
        }
        if !dist[end].is_finite() {
            return None;
        }
        let mut path = vec![end];
        while *path.last().expect("path starts non-empty") != start {
            path.push(prev[*path.last().expect("path starts non-empty")]);
        }
        path.reverse();
        Some((dist[end], path))
    }

    /// Whether at least two different routes (never visiting an airport twice) of hops join
    /// `start` and `end`. They do exactly when a route between them uses a flight
    /// that isn't a bridge: such a flight lies on a loop, whose other side is a
    /// second way through, while a route made only of bridges is the only one.
    /// Every route between two airports crosses the same bridges, so any one will do.
    pub fn has_two_routes(&self, start: usize, end: usize) -> bool {
        self.shortest(start, end)
            .is_some_and(|(_, path)| self.crosses_a_loop(&path))
    }

    /// Whether a route (as its airports) uses a hop that isn't a bridge, which
    /// is what [`Network::has_two_routes`] checks.
    fn crosses_a_loop(&self, path: &[usize]) -> bool {
        path.windows(2)
            .any(|w| !self.bridges.contains(&(w[0].min(w[1]), w[0].max(w[1]))))
    }

    /// Every airport that makes a puzzle with `start` within `max_hops` (any
    /// number for `None`), in index order. One shortest-route search from `start`
    /// settles them all, carrying along each route's hops and whether it has
    /// crossed a non-bridge hop, as `scripts/build-flights-data.py` counts them.
    pub fn puzzle_ends(&self, start: usize, max_hops: Option<usize>) -> Vec<usize> {
        let n = self.len();
        let mut dist = vec![f64::INFINITY; n];
        let mut hops = vec![0usize; n];
        let mut two = vec![false; n];
        let mut done = vec![false; n];
        let mut heap = BinaryHeap::new();
        dist[start] = 0.0;
        heap.push(Entry(0.0, start));
        while let Some(Entry(d, node)) = heap.pop() {
            if done[node] {
                continue;
            }
            done[node] = true;
            for &Hop {
                to: next, distance, ..
            } in &self.adj[node]
            {
                let nd = d + distance;
                if nd < dist[next] {
                    dist[next] = nd;
                    hops[next] = hops[node] + 1;
                    two[next] =
                        two[node] || !self.bridges.contains(&(node.min(next), node.max(next)));
                    heap.push(Entry(nd, next));
                }
            }
        }
        let code = self.code(start);
        (0..n)
            .filter(|&end| {
                done[end]
                    && two[end]
                    && letters_apart(code, self.code(end)) == 3
                    && max_hops.is_none_or(|max| hops[end] <= max)
            })
            .collect()
    }

    /// Number of hops in the shortest route from `start` to `end`.
    pub fn best_hops(&self, start: usize, end: usize) -> Option<usize> {
        self.shortest(start, end).map(|(_, path)| path.len() - 1)
    }

    /// Number of valid puzzles whose shortest route has at most `max_hops` hops
    /// (all of them for `None`), from the counts in the data file.
    pub fn puzzle_count(&self, max_hops: Option<usize>) -> usize {
        self.puzzle_counts
            .iter()
            .filter(|&&(hops, _)| max_hops.is_none_or(|max| hops <= max))
            .map(|&(_, count)| count)
            .sum()
    }

    /// Whether `start` to `end` makes a puzzle: the codes differ in every letter (so
    /// it takes at least three flights) and at least two routes join them.
    pub fn is_valid_puzzle(&self, start: usize, end: usize) -> bool {
        letters_apart(self.code(start), self.code(end)) == 3 && self.has_two_routes(start, end)
    }

    /// A random puzzle whose shortest route has at most `max_hops` hops (any
    /// number for `None`): a random start, then a random end among the airports it
    /// can reach in that many hops. Drawn fresh each time rather than from a
    /// precomputed list, since the pairs number in the millions.
    pub fn random_puzzle(
        &self,
        rng: &mut fastrand::Rng,
        max_hops: Option<usize>,
    ) -> Option<(usize, usize)> {
        for _ in 0..PUZZLE_ATTEMPTS {
            let start = rng.usize(0..self.len());
            if self.adj[start].is_empty() {
                continue;
            }
            if let Some(end) = self.random_end(rng, start, max_hops, END_ATTEMPTS) {
                return Some((start, end));
            }
        }
        // Random draws miss only when puzzles are rare (a custom game allowing no
        // connections, say), so then every start is tried, in random order.
        let mut starts: Vec<usize> = (0..self.len())
            .filter(|&i| !self.adj[i].is_empty())
            .collect();
        rng.shuffle(&mut starts);
        starts.into_iter().find_map(|start| {
            self.random_end(rng, start, max_hops, usize::MAX)
                .map(|end| (start, end))
        })
    }

    /// A random end that makes a puzzle from `start` within `max_hops`, trying up
    /// to `attempts` of the airports within reach (each needs a shortest-route
    /// search, so a few random ones are tried rather than all).
    fn random_end(
        &self,
        rng: &mut fastrand::Rng,
        start: usize,
        max_hops: Option<usize>,
        attempts: usize,
    ) -> Option<usize> {
        // An end within reach in `max_hops` can still be too far by the shortest
        // route, which may take more hops than the fewest; it's skipped then.
        let code = self.code(start);
        let mut ends: Vec<usize> = self
            .within(start, max_hops)
            .into_iter()
            .filter(|&end| letters_apart(code, self.code(end)) == 3)
            .collect();
        rng.shuffle(&mut ends);
        ends.into_iter().take(attempts).find(|&end| {
            // One search gives both the route's length and whether it has a loop.
            self.shortest(start, end).is_some_and(|(_, path)| {
                max_hops.is_none_or(|max| path.len() - 1 <= max) && self.crosses_a_loop(&path)
            })
        })
    }

    /// The puzzle for a UTC day (days since 1970-01-01), the same for everyone on
    /// the same build.
    pub fn daily_puzzle(&self, utc_day: u64, max_hops: Option<usize>) -> Option<(usize, usize)> {
        let day = utc_day.saturating_sub(DAILY_EPOCH_DAY);
        let mut rng =
            fastrand::Rng::with_seed(DAILY_SEED ^ day.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        self.random_puzzle(&mut rng, max_hops)
    }

    /// Airports reachable from `start` in at most `max_hops` hops (any number for
    /// `None`), itself included, in index order.
    fn within(&self, start: usize, max_hops: Option<usize>) -> Vec<usize> {
        let mut depth = vec![usize::MAX; self.len()];
        depth[start] = 0;
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            if max_hops.is_some_and(|max| depth[node] >= max) {
                continue;
            }
            for &Hop { to: next, .. } in &self.adj[node] {
                if depth[next] == usize::MAX {
                    depth[next] = depth[node] + 1;
                    queue.push_back(next);
                }
            }
        }
        (0..self.len())
            .filter(|&i| depth[i] != usize::MAX)
            .collect()
    }

    /// Bridges as `(lower, higher)` index pairs, by Tarjan's low-link method
    /// (iterative, since the network's chains could overflow a recursive one).
    fn find_bridges(&self) -> HashSet<(usize, usize)> {
        let n = self.len();
        let mut disc = vec![usize::MAX; n];
        let mut low = vec![0; n];
        let mut out = HashSet::new();
        let mut timer = 0;
        for root in 0..n {
            if disc[root] != usize::MAX {
                continue;
            }
            disc[root] = timer;
            low[root] = timer;
            timer += 1;
            // (node, parent, next neighbour position)
            let mut stack = vec![(root, usize::MAX, 0usize)];
            while let Some(&mut (node, parent, ref mut pos)) = stack.last_mut() {
                if let Some(&Hop { to: next, .. }) = self.adj[node].get(*pos) {
                    *pos += 1;
                    if next == parent {
                        continue;
                    }
                    if disc[next] == usize::MAX {
                        disc[next] = timer;
                        low[next] = timer;
                        timer += 1;
                        stack.push((next, node, 0));
                    } else {
                        low[node] = low[node].min(disc[next]);
                    }
                } else {
                    stack.pop();
                    if parent != usize::MAX {
                        low[parent] = low[parent].min(low[node]);
                        if low[node] > disc[parent] {
                            out.insert((node.min(parent), node.max(parent)));
                        }
                    }
                }
            }
        }
        out
    }
}
