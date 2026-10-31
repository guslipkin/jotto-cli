use std::time::Instant;

use jotto::flights::data::{network, Network, STANDARD_CONNECTIONS};
use jotto::flights::game::{
    Difficulty, FlightsGame, FlyError, FlyOutcome, PuzzleError, Rules, SavedRules, Status, Units,
    DAILY_DIFFICULTY,
};

/// A small network: AAA to BBB along the equator through ABA and ABB (3° in all),
/// or the long way round through BAA and BBA; AAB hangs off AAA through a
/// connection at ZZZ, so only with one connection or more.
const FIXTURE: &str = "\
# fixture
A\tAAA\t0\t0\tAlpha\tAA
A\tAAB\t0\t-1\t\tAB
A\tABA\t0\t1\tAbacus
A\tABB\t0\t2
A\tBAA\t1\t0
A\tBBA\t1\t1.5
A\tBBB\t0\t3
A\tZZZ\t0.5\t-0.5
H\tAAA\tAAB\tx\tZZZ\tZZZ
H\tAAA\tABA\t-\t-\t-
H\tABA\tABB\t-\t-\t-
H\tABB\tBBB\t-\t-\t-
H\tAAA\tBAA\t-\t-\t-
H\tBAA\tBBA\t-\t-\t-
H\tBBA\tBBB\t-\t-\t-
P\t0\t3\t2
P\t1\t3\t2
P\t2\t3\t2
";

fn fixture_with(max_connections: usize) -> &'static Network {
    Box::leak(Box::new(Network::parse(FIXTURE, max_connections).unwrap()))
}

fn fixture() -> &'static Network {
    fixture_with(STANDARD_CONNECTIONS)
}

fn idx(net: &Network, code: &str) -> usize {
    net.index_of(code).unwrap()
}

fn game_at(net: &'static Network, difficulty: Difficulty) -> FlightsGame {
    FlightsGame::with_puzzle(
        net,
        idx(net, "AAA"),
        idx(net, "BBB"),
        difficulty,
        Rules::default(),
        Units::Nm,
        None,
    )
    .unwrap()
}

fn game(net: &'static Network) -> FlightsGame {
    game_at(net, Difficulty::Medium)
}

#[test]
fn shortest_route_is_by_distance() {
    let net = fixture();
    let (dist, path) = net.shortest(idx(net, "AAA"), idx(net, "BBB")).unwrap();
    let codes: Vec<&str> = path.iter().map(|&i| net.code(i)).collect();
    assert_eq!(codes, ["AAA", "ABA", "ABB", "BBB"]);
    // 3 degrees of the equator is 180 nautical miles, near enough.
    assert!((dist - 180.0).abs() < 1.0, "{dist}");
}

#[test]
fn connections_count_toward_distance_and_reverse() {
    let net = fixture();
    let (aaa, aab, zzz) = (idx(net, "AAA"), idx(net, "AAB"), idx(net, "ZZZ"));
    let there = net.hop(aaa, aab).unwrap();
    assert_eq!(there.via, [zzz]);
    // Through ZZZ is longer than AAB's straight 60 nm.
    assert!(there.distance > 80.0, "{}", there.distance);
    let back = net.hop(aab, aaa).unwrap();
    assert_eq!(back.via, [zzz]);
    assert_eq!(back.distance, there.distance);
    // Connection-only airports have no hops of their own.
    assert!(net.hops(zzz).is_empty());
}

#[test]
fn connection_limit_picks_the_itinerary() {
    let direct_only = fixture_with(0);
    assert_eq!(direct_only.max_connections(), 0);
    assert!(direct_only
        .hop(idx(direct_only, "AAA"), idx(direct_only, "AAB"))
        .is_none());
    let mut g = game(direct_only);
    assert_eq!(
        g.fly("AAB"),
        Err(FlyError::NoTicket {
            from: "AAA".to_string(),
            to: "AAB".to_string(),
            max_connections: 0
        })
    );
    assert!(g
        .fly("AAB")
        .unwrap_err()
        .to_string()
        .contains("no direct flight"));
    assert!(fixture_with(2)
        .hop(idx(direct_only, "AAA"), idx(direct_only, "AAB"))
        .is_some());
    assert!(Network::parse(FIXTURE, 3).is_err());
}

#[test]
fn places_read_city_and_country() {
    let net = fixture();
    assert_eq!(net.place(idx(net, "AAA")), "Alpha, AA");
    assert_eq!(net.place(idx(net, "AAB")), "AB");
    assert_eq!(net.place(idx(net, "ABA")), "Abacus");
    assert_eq!(net.place(idx(net, "ABB")), "");
    let real = network(STANDARD_CONNECTIONS);
    assert_eq!(real.place(real.index_of("LAX").unwrap()), "Los Angeles, US");
}

#[test]
fn puzzles_need_three_letters_and_two_routes() {
    let net = fixture();
    let (aaa, bbb, aab, abb) = (
        idx(net, "AAA"),
        idx(net, "BBB"),
        idx(net, "AAB"),
        idx(net, "ABB"),
    );
    assert!(net.has_two_routes(aaa, bbb));
    assert!(net.is_valid_puzzle(aaa, bbb));
    // AAB's only hop is a bridge, so there's one route to it.
    assert!(!net.has_two_routes(aaa, aab));
    // Two routes, but ABB is only two letters from AAA.
    assert!(net.has_two_routes(aaa, abb));
    assert!(!net.is_valid_puzzle(aaa, abb));
    let mut rng = fastrand::Rng::with_seed(7);
    for _ in 0..20 {
        let (s, e) = net.random_puzzle(&mut rng, None).unwrap();
        assert!(net.is_valid_puzzle(s, e));
    }
    // The only puzzle is AAA-BBB (and back), whose shortest route is 3 hops.
    let (s, e) = net.random_puzzle(&mut rng, Some(3)).unwrap();
    assert_eq!(net.best_hops(s, e), Some(3));
    // A network with no puzzle says so rather than looping or panicking.
    let line = "A\tAAA\t0\t0\nA\tAAB\t0\t1\nH\tAAA\tAAB\t-\t-\t-\n";
    let line: &'static Network = Box::leak(Box::new(Network::parse(line, 1).unwrap()));
    assert_eq!(line.random_puzzle(&mut rng, None), None);
    assert!(matches!(
        FlightsGame::random(Difficulty::Custom, Rules::default(), Units::Nm, Some(line)),
        Err(PuzzleError::NoPuzzles(_))
    ));
}

#[test]
fn rejects_bad_moves() {
    let net = fixture();
    let mut g = game(net);
    assert_eq!(g.fly(""), Err(FlyError::Empty));
    assert!(matches!(g.fly("ab"), Err(FlyError::NotACode(_))));
    assert!(matches!(g.fly("A1A"), Err(FlyError::NotACode(_))));
    assert!(matches!(g.fly("QQQ"), Err(FlyError::UnknownAirport(_))));
    assert!(matches!(g.fly("AAA"), Err(FlyError::AlreadyOnRoute(_))));
    assert!(matches!(g.fly("BBB"), Err(FlyError::NotOneLetter { .. })));
    assert!(matches!(g.fly("aab"), Ok(FlyOutcome::Flew { .. })));
    // AAB and ABB are one letter apart, but there's no ticket between them.
    assert!(matches!(g.fly("ABB"), Err(FlyError::NoTicket { .. })));
    assert!(matches!(g.fly("AAA"), Err(FlyError::AlreadyOnRoute(_))));
}

#[test]
fn options_skip_the_route() {
    let net = fixture();
    let mut g = game(net);
    let codes: Vec<&str> = g.options().iter().map(|o| o.code).collect();
    assert_eq!(codes, ["AAB", "ABA", "BAA"]);
    let aab = g.options().into_iter().find(|o| o.code == "AAB").unwrap();
    assert_eq!(aab.via, ["ZZZ"]);
    assert_eq!(aab.changed, 2);
    g.fly("ABA").unwrap();
    let codes: Vec<&str> = g.options().iter().map(|o| o.code).collect();
    assert_eq!(codes, ["ABB"]);
}

#[test]
fn shortest_route_wins() {
    let net = fixture();
    let mut g = game(net);
    g.fly("ABA").unwrap();
    g.fly("ABB").unwrap();
    assert_eq!(g.fly("BBB"), Ok(FlyOutcome::Won));
    assert_eq!(g.status(), Status::Won);
    assert_eq!(g.hops(), 3);
    assert!(g.share_summary().contains("first try"));
    assert_eq!(g.fly("ABB"), Err(FlyError::GameOver));
}

#[test]
fn longer_route_plays_on_and_back_undoes() {
    let net = fixture();
    let mut g = game(net);
    // A detour that's backed out of doesn't count.
    g.fly("AAB").unwrap();
    assert_eq!(g.back(), Some(idx(net, "AAB")));
    assert_eq!(g.total_distance(), 0.0);
    g.fly("BAA").unwrap();
    g.fly("BBA").unwrap();
    let Ok(FlyOutcome::Longer { over }) = g.fly("BBB") else {
        panic!("the long way round should be longer");
    };
    assert!(over > 0.0);
    assert!((g.total_distance() - g.best_distance() - over).abs() < 1e-9);
    assert_eq!(g.status(), Status::Playing);
    assert!(g.at_destination());
    assert!(g.options().is_empty());
    assert_eq!(g.fly("ABB"), Err(FlyError::AtDestination));
    // Back to the start, one hop at a time, and no further.
    for _ in 0..3 {
        assert!(g.back().is_some());
    }
    assert_eq!(g.back(), None);
    assert_eq!(g.route(), [idx(net, "AAA")]);
    for code in ["ABA", "ABB", "BBB"] {
        g.fly(code).unwrap();
    }
    assert_eq!(g.status(), Status::Won);
    assert_eq!(g.longer_arrivals(), 1);
    assert!(g.share_summary().contains("after 1 longer route"));
}

#[test]
fn give_up_reveals_best_route() {
    let net = fixture();
    let mut g = game(net);
    g.give_up();
    assert_eq!(g.status(), Status::GivenUp);
    assert_eq!(g.route_text(g.best_route()), "AAA → ABA → ABB → BBB");
    assert_eq!(g.back(), None);
}

#[test]
fn units_convert_and_format() {
    assert_eq!(Units::Nm.format(1234.4), "1,234 nm");
    assert_eq!(Units::Km.format(100.0), "185 km");
    assert_eq!(Units::Mi.format(100.0), "115 mi");
    assert_eq!("km".parse::<Units>(), Ok(Units::Km));
    assert!("parsecs".parse::<Units>().is_err());
    assert_eq!(Units::Km.next().next(), Units::Mi);
}

#[test]
fn difficulty_levels() {
    assert_eq!("pilot".parse::<Difficulty>(), Ok(Difficulty::Pilot));
    assert_eq!("custom".parse::<Difficulty>(), Ok(Difficulty::Custom));
    assert_eq!(Difficulty::Hard.next(), Difficulty::Pilot);
    assert_eq!(Difficulty::Pilot.next(), Difficulty::Custom);
    assert_eq!(Difficulty::Custom.next(), Difficulty::Easy);
    let rules = |d: Difficulty| d.rules(Rules::default());
    assert!(rules(Difficulty::Easy).shows_distances());
    assert!(!rules(Difficulty::Medium).shows_distances());
    assert!(rules(Difficulty::Medium).shows_map());
    assert!(rules(Difficulty::Medium).shows_connections());
    assert!(rules(Difficulty::Medium).shows_amount_over());
    assert!(!rules(Difficulty::Hard).shows_map());
    assert!(!rules(Difficulty::Hard).shows_amount_over());
    assert!(rules(Difficulty::Hard).shows_connections());
    assert!(!rules(Difficulty::Pilot).shows_connections());
    let caps: Vec<_> = Difficulty::STANDARD
        .iter()
        .map(|&d| rules(d).max_hops)
        .collect();
    assert_eq!(caps, [Some(4), Some(6), Some(8), None]);
    for d in Difficulty::STANDARD {
        assert_eq!(rules(d).max_connections, STANDARD_CONNECTIONS);
        assert!(rules(d).show_places);
    }
    // Custom plays by whatever it's given.
    let mine = Rules {
        view: Difficulty::Pilot,
        max_hops: Some(9),
        max_connections: 0,
        show_places: false,
        fixed_route: false,
        start: None,
        end: None,
    };
    assert_eq!(Difficulty::Custom.rules(mine), mine);
}

#[test]
fn saved_rules_round_trip_and_reject_nonsense() {
    let mine = Rules {
        view: Difficulty::Hard,
        max_hops: None,
        max_connections: 2,
        show_places: false,
        fixed_route: false,
        start: None,
        end: None,
    };
    assert_eq!(SavedRules::from(mine).rules(), Some(mine));
    for broken in [
        SavedRules {
            view: "custom".into(),
            ..mine.into()
        },
        SavedRules {
            max_hops: Some(2),
            ..mine.into()
        },
        SavedRules {
            max_hops: Some(21),
            ..mine.into()
        },
        SavedRules {
            max_connections: 3,
            ..mine.into()
        },
    ] {
        assert_eq!(broken.rules(), None, "{broken:?}");
    }
}

#[test]
fn rejects_malformed_data() {
    for bad in [
        "A\tAAA\t0\t0\nH\tAAA\tABA\t-\t-\t-\n", // unlisted airport
        "A\tBBB\t0\t0\nA\tAAA\t0\t0\n",         // unsorted
        "A\tAAA\t0\t0\nA\tBBB\t0\t0\nH\tAAA\tBBB\t-\t-\t-\n", // three letters apart
        "A\tAAA\t0\t0\nA\tAAB\t0\t0\nH\tAAA\tAAB\t-\n", // one itinerary, not three
        // Two connections where at most one is allowed.
        "A\tAAA\t0\t0\nA\tAAB\t0\t0\nA\tXXX\t0\t0\nA\tYYY\t0\t0\nH\tAAA\tAAB\tx\tXXX,YYY\tXXX\n",
        "A\tAAA\tx\t0\n",
        "X\tAAA\n",
    ] {
        assert!(Network::parse(bad, 1).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn real_network_makes_puzzles_quickly() {
    let net = network(STANDARD_CONNECTIONS);
    assert!(net.len() >= 2_000, "only {} airports", net.len());
    assert!(net.hop_count() >= 4_000, "only {} hops", net.hop_count());
    let mut rng = fastrand::Rng::with_seed(1);
    let started = Instant::now();
    for d in Difficulty::STANDARD {
        let max_hops = d.rules(Rules::default()).max_hops;
        for _ in 0..10 {
            let (s, e) = net.random_puzzle(&mut rng, max_hops).unwrap();
            assert!(net.is_valid_puzzle(s, e));
            let hops = net.best_hops(s, e).unwrap();
            assert!(hops >= 3);
            assert!(max_hops.is_none_or(|max| hops <= max), "{d}: {hops} hops");
        }
    }
    let per = started.elapsed() / 40;
    assert!(per.as_millis() < 250, "a puzzle took {per:?}");
}

#[test]
fn rare_puzzles_are_still_found() {
    // With no connections there are only a few dozen puzzles in all.
    let net = network(0);
    let mut rng = fastrand::Rng::with_seed(3);
    let started = Instant::now();
    let (s, e) = net.random_puzzle(&mut rng, Some(3)).unwrap();
    assert_eq!(net.best_hops(s, e), Some(3));
    assert!(
        started.elapsed().as_secs() < 5,
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn puzzle_counts_grow_with_difficulty() {
    let counts: Vec<usize> = Difficulty::STANDARD
        .iter()
        .map(|d| d.puzzle_count())
        .collect();
    assert!(counts[0] >= 10_000, "only {} easy puzzles", counts[0]);
    assert!(counts.windows(2).all(|w| w[0] < w[1]), "{counts:?}");
    assert_eq!(counts[3], network(STANDARD_CONNECTIONS).puzzle_count(None));
    // More connections, more puzzles.
    let by_connections: Vec<usize> = (0..=2).map(|k| network(k).puzzle_count(None)).collect();
    assert!(
        by_connections.windows(2).all(|w| w[0] < w[1]),
        "{by_connections:?}"
    );
}

#[test]
fn games_are_dealt_within_their_level() {
    for d in Difficulty::STANDARD {
        let g = FlightsGame::new(d, Units::Nm);
        let hops = g.best_route().len() - 1;
        let max = d.rules(Rules::default()).max_hops;
        assert!(max.is_none_or(|max| hops <= max), "{d}: {hops} hops");
        assert_eq!(g.network().max_connections(), STANDARD_CONNECTIONS);
    }
    let daily = FlightsGame::new_daily(Rules::default(), Units::Nm);
    assert!(daily.best_route().len() - 1 <= 6);
}

#[test]
fn custom_games_play_by_their_rules() {
    let mine = Rules {
        view: Difficulty::Pilot,
        max_hops: Some(4),
        max_connections: 2,
        show_places: false,
        fixed_route: false,
        start: None,
        end: None,
    };
    let mut g = FlightsGame::random(Difficulty::Custom, mine, Units::Nm, None).unwrap();
    assert_eq!(g.rules(), mine);
    assert_eq!(g.network().max_connections(), 2);
    assert!(g.best_route().len() - 1 <= 4);
    // Changing the settings leaves the game being played alone...
    let theirs = Rules {
        max_connections: 0,
        max_hops: Some(3),
        ..mine
    };
    g.set_custom(theirs);
    assert_eq!(g.rules(), mine);
    // ...and it resumes as it began.
    let first = g.options()[0].code;
    g.fly(first).unwrap();
    let back = FlightsGame::from_saved(&g.to_saved()).unwrap().unwrap();
    assert_eq!(back.rules(), mine);
    assert_eq!(back.custom(), theirs);
    assert_eq!(back.route(), g.route());
    // The next custom game uses the new settings, on the network they need.
    g.reset(Some(Difficulty::Custom)).unwrap();
    assert_eq!(g.rules(), theirs);
    assert_eq!(g.network().max_connections(), 0);
    // A standard level after it is back to standard rules.
    g.reset(Some(Difficulty::Easy)).unwrap();
    assert_eq!(g.network().max_connections(), STANDARD_CONNECTIONS);
    assert_eq!(g.custom(), theirs);
}

#[test]
fn distances_show_on_easy_or_when_over() {
    let net = fixture();
    let mut g = game_at(net, Difficulty::Hard);
    assert!(!g.distances_visible());
    g.give_up();
    assert!(g.distances_visible());
    assert!(game(net).difficulty() == Difficulty::Medium && !game(net).distances_visible());
}

#[test]
fn daily_is_deterministic_and_medium() {
    let day = 20_400;
    let a = FlightsGame::daily_in(None, day, Rules::default(), Units::Km);
    // Custom settings don't change the daily puzzle.
    let b = FlightsGame::daily_in(
        None,
        day,
        Rules {
            max_connections: 0,
            ..Rules::default()
        },
        Units::Nm,
    );
    assert_eq!((a.origin(), a.destination()), (b.origin(), b.destination()));
    assert_eq!(a.difficulty(), DAILY_DIFFICULTY);
    assert_eq!(a.daily_date(), Some("2025-11-08"));
    let days: std::collections::HashSet<_> = (0..10)
        .map(|d| network(STANDARD_CONNECTIONS).daily_puzzle(day + d, Some(6)))
        .collect();
    assert!(days.len() >= 9, "daily puzzles repeat");
}

#[test]
fn custom_route_and_save_round_trip() {
    let mut g = FlightsGame::new(Difficulty::Easy, Units::Mi);
    let custom = format!("{}-{}", g.start_code(), g.end_code());
    let again = FlightsGame::new_route(
        &custom.to_lowercase(),
        Difficulty::Hard,
        Rules::default(),
        Units::Km,
    )
    .unwrap();
    assert_eq!(
        (again.origin(), again.destination()),
        (g.origin(), g.destination())
    );
    for bad in ["LAX", "QQQ-LAX"] {
        assert!(
            FlightsGame::new_route(bad, Difficulty::Hard, Rules::default(), Units::Km).is_err()
        );
    }

    let first = g.options()[0].code;
    g.fly(first).unwrap();
    let saved = g.to_saved();
    assert_eq!(saved.units, "mi");
    let json = serde_json::to_string(&saved).unwrap();
    let back = FlightsGame::from_saved(&serde_json::from_str(&json).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(back.route(), g.route());
    assert_eq!(back.difficulty(), Difficulty::Easy);
    assert_eq!(back.units(), Units::Mi);

    // A save whose route can't be flown is refused, not half-loaded.
    let mut broken = saved.clone();
    broken.game.as_mut().unwrap().route = vec!["QQQ".to_string()];
    assert!(FlightsGame::from_saved(&broken).is_err());

    // A finished game isn't saved, but the units are.
    g.give_up();
    let done = g.to_saved();
    assert!(done.game.is_none());
    assert_eq!(done.units, "mi");
}

#[test]
fn destination_comes_first_in_the_options() {
    let net = fixture();
    // ABA is one hop from AAA; as the destination it heads the list.
    let g = FlightsGame::with_puzzle(
        net,
        idx(net, "AAA"),
        idx(net, "ABA"),
        Difficulty::Medium,
        Rules::default(),
        Units::Nm,
        None,
    )
    .unwrap();
    let codes: Vec<&str> = g.options().iter().map(|o| o.code).collect();
    assert_eq!(codes, ["ABA", "AAB", "BAA"]);
}

#[test]
fn custom_start_and_end_are_checked() {
    let with = |start: Option<&str>, end: Option<&str>, max_hops| Rules {
        view: Difficulty::Medium,
        max_hops,
        max_connections: STANDARD_CONNECTIONS,
        show_places: true,
        fixed_route: true,
        start: start.map(|c| *<&[u8; 3]>::try_from(c.as_bytes()).unwrap()),
        end: end.map(|c| *<&[u8; 3]>::try_from(c.as_bytes()).unwrap()),
    };
    // A start alone deals a random puzzle from it, as an end alone does to it.
    let from = with(Some("LAX"), None, Some(6));
    assert!(from.problem().is_none());
    assert!(from.puzzle_count() > 0);
    let g = FlightsGame::random(Difficulty::Custom, from, Units::Nm, None).unwrap();
    assert_eq!(g.start_code(), "LAX");
    let to = with(None, Some("LAX"), None);
    let g = FlightsGame::random(Difficulty::Custom, to, Units::Nm, None).unwrap();
    assert_eq!(g.end_code(), "LAX");
    // Both: the pair is the puzzle, if it is one.
    let both = with(Some("LAX"), Some("JFK"), None);
    assert_eq!(both.puzzle_count(), 1);
    let g = FlightsGame::random(Difficulty::Custom, both, Units::Nm, None).unwrap();
    assert_eq!((g.start_code(), g.end_code()), ("LAX", "JFK"));
    // A fixed route isn't held to the limits for random puzzles: LAX to JFK
    // takes 9 hops, and the route uses the standard connections.
    let long = Rules {
        max_connections: 0,
        ..with(Some("LAX"), Some("JFK"), Some(4))
    };
    assert!(long.problem().is_none(), "{:?}", long.problem());
    assert_eq!(long.network().max_connections(), STANDARD_CONNECTIONS);
    // Random puzzles ignore a start and end left from a fixed route.
    let random = Rules {
        fixed_route: false,
        ..with(Some("LAX"), Some("LAS"), Some(4))
    };
    assert!(random.problem().is_none());
    let g = FlightsGame::random(Difficulty::Custom, random, Units::Nm, None).unwrap();
    assert!(g.best_route().len() - 1 <= 4);
    for (rules, why) in [
        (
            with(Some("LAX"), Some("LAS"), None),
            "differ in every letter",
        ),
        (with(Some("QQQ"), None, None), "QQQ has no hops"),
    ] {
        let problem = rules.problem().unwrap();
        assert!(problem.contains(why), "{problem}");
        assert_eq!(rules.puzzle_count(), 0);
        let err = FlightsGame::random(Difficulty::Custom, rules, Units::Nm, None).unwrap_err();
        assert!(err.to_string().contains(why), "{err}");
    }
    // Kept in the save as codes.
    let saved = SavedRules::from(both);
    assert_eq!(saved.start.as_deref(), Some("LAX"));
    assert_eq!(saved.rules(), Some(both));
    assert_eq!(
        SavedRules {
            start: Some("LA".into()),
            ..saved
        }
        .rules(),
        None
    );
}
