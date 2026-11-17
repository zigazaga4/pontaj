//! Place names as the ALT app writes them ("Aeroport Otopeni Bucuresti", "Navodari", "Mamaia Nord"…).

/// Lower case, without diacritics, punctuation turned into single spaces: "Năvodari" → "navodari".
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'ă' | 'â' => 'a',
            'î' => 'i',
            'ș' | 'ş' => 's',
            'ț' | 'ţ' => 't',
            other => other,
        };
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

/// Names that put a place at the Bucharest end: the city, the airports and the hotel next to Otopeni airport (the API
/// sometimes writes only "Hilton Garden Inn", without the town).
const BUCHAREST: [&str; 7] = ["otopeni", "bucuresti", "bucharest", "baneasa", "banea", "henri coanda", "hilton garden"];
const CONSTANTA: [&str; 2] = ["constanta", "mamaia"];

/// Where a place is, as far as "did the driver stay where he arrived" is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Area {
    Constanta,
    Bucuresti,
    Other(String),
}

/// The area of a place. Anywhere in Constanța county counts as the Constanța side (`extra` are the places the office added).
pub fn area(name: &str, extra: &[String]) -> Area {
    let normalized = normalize(name);
    if BUCHAREST.iter().any(|k| normalized.contains(k)) {
        Area::Bucuresti
    } else if CONSTANTA.iter().any(|k| normalized.contains(k)) || in_county(name, extra) {
        Area::Constanta
    } else {
        Area::Other(normalized)
    }
}

pub fn same_area(a: &str, b: &str, extra: &[String]) -> bool {
    area(a, extra) == area(b, extra)
}

/// Is the place at the Bucharest end of the regular route (the airports and the city)?
pub fn is_bucharest(name: &str) -> bool {
    let normalized = normalize(name);
    BUCHAREST.iter().any(|k| normalized.contains(k))
}

/// The two ends of the regular route: Constanța itself and Bucharest (its airports included).
/// Resorts such as Mamaia are not part of it: they have their own tariff.
pub fn is_main_route(name: &str) -> bool {
    let normalized = normalize(name);
    normalized == "constanta" || BUCHAREST.iter().any(|k| normalized.contains(k))
}

/// Towns and resorts of Constanța county whose name can be followed by a qualifier ("Mamaia Nord", "Eforie Sud", "Mangalia Port").
const COUNTY_TOWNS: &[&str] = &[
    "constanta", "mamaia", "navodari", "eforie", "mangalia", "techirghiol", "medgidia", "cernavoda", "harsova", "hirsova", "ovidiu", "neptun", "olimp", "costinesti", "agigea",
    "murfatlar",
];

/// The other places of Constanța county, to be written exactly (a loose match would turn "Poiana Brașov" into a local place).
const COUNTY_PLACES: &[&str] = &[
    "2 mai", "23 august", "adamclisi", "albesti", "aliman", "amzacea", "baraganu", "basarabi", "castelu", "cerchezu", "ciobanu", "ciocarlia", "cobadin", "cogealac", "comana", "corbu",
    "crucea", "cumpana", "cuza voda", "deleni", "dobromir", "dumbraveni", "fantanele", "garliciu", "ghindaresti", "gradina", "horia", "independenta", "ion corvin", "istria",
    "jupiter", "lanurile", "lazu", "limanu", "lipnita", "lumina", "mereni", "mihai viteazu", "mihail kogalniceanu", "aeroport mihail kogalniceanu", "mircea voda", "negru voda",
    "nicolae balcescu", "oltina", "ostrov", "palazu mare", "pantelimon", "pecineaga", "pestera", "poarta alba", "poiana", "rasova", "sacele", "saligny", "saraiu", "seimeni",
    "siutghiol", "targusor", "topalu", "topraisar", "tortoman", "tuzla", "valu lui traian", "valu traian", "valu", "kogalniceanu", "vama veche", "vadu", "venus", "saturn", "aurora",
    "vulturu",
];

fn starts_with_entry(normalized: &str, entry: &str) -> bool {
    normalized == entry || normalized.strip_prefix(entry).is_some_and(|rest| rest.starts_with(' '))
}

/// Is the place in Constanța county? `extra` are places the office added in the settings (written like the API writes them).
pub fn in_county(name: &str, extra: &[String]) -> bool {
    let normalized = normalize(name);
    COUNTY_TOWNS.iter().any(|t| starts_with_entry(&normalized, t))
        || COUNTY_PLACES.contains(&normalized.as_str())
        || extra.iter().map(|e| normalize(e)).filter(|e| !e.is_empty()).any(|e| starts_with_entry(&normalized, &e))
}

/// Is the place somewhere the driver goes during an ordinary working day: the regular route (Constanța, Bucharest)
/// or Constanța county? Anything else is far away: an excursion.
pub fn is_local(name: &str, extra: &[String]) -> bool {
    is_main_route(name) || in_county(name, extra)
}

/// Does a trip `from → to` belong to the regular route? Constanța ↔ Bucharest, and also any "Otopeni run": a trip with
/// the Bucharest end (Otopeni, the airports) on one side and any place on the other, even outside Constanța county
/// (Otopeni → Năvodari, Mamaia → Otopeni, Tulcea → Otopeni): it is an airport run like the scheduled ones.
pub fn is_regular_route(from: &str, to: &str) -> bool {
    (is_main_route(from) && is_main_route(to)) || is_bucharest(from) || is_bucharest(to)
}

/// Edit distance between two short strings.
pub fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (row[j] + 1).min(above + 1).min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize("Năvodari"), "navodari");
        assert_eq!(normalize("  Aeroport Mihail Kogălniceanu! "), "aeroport mihail kogalniceanu");
        assert_eq!(normalize("Constanța–Jurilovca"), "constanta jurilovca");
    }

    #[test]
    fn areas_and_main_route() {
        let none: &[String] = &[];
        assert!(same_area("Aeroport Otopeni Bucuresti", "Bucuresti", none));
        assert!(same_area("Aeroport Banea", "Otopeni", none));
        assert!(same_area("Mamaia Nord", "Constanta", none));
        assert!(same_area("Hotel Scandinavia Mamaia", "Constanta", none));
        assert!(!same_area("Constanta", "Aeroport Otopeni Bucuresti", none));
        assert!(!same_area("Constanta", "Tulcea", none));
        assert!(is_main_route("Constanta") && is_main_route("Aeroport Otopeni Bucuresti"));
        assert!(!is_main_route("Mamaia Nord") && !is_main_route("Tulcea") && !is_main_route("Chitila"));
    }

    #[test]
    fn the_whole_county_is_the_constanta_side_of_a_pause() {
        let none: &[String] = &[];
        // He arrived in Năvodari and the next run leaves from Constanța: he did not change area.
        assert!(same_area("Navodari", "Constanta", none));
        assert!(same_area("Cernavoda", "Constanta", none));
        assert!(!same_area("Navodari", "Aeroport Otopeni Bucuresti", none));
        // The office can add a place.
        let extra = vec!["Chitila".to_string()];
        assert!(!same_area("Chitila", "Constanta", none));
        assert!(same_area("Chitila", "Constanta", &extra));
    }

    #[test]
    fn any_trip_with_the_otopeni_end_is_on_the_regular_route() {
        for (from, to) in [
            ("Constanta", "Aeroport Otopeni Bucuresti"),
            ("Aeroport Otopeni Bucuresti", "Navodari"),
            ("Aeroport Otopeni Bucuresti", "Mamaia Nord"),
            ("Cernavoda", "Aeroport Otopeni Bucuresti"),
            ("Valu Traian", "Otopeni"),
            ("Aeroport Banea", "Eforie Nord-constanta"),
            // Outside the county, toward or from Otopeni, is the same kind of run.
            ("Tulcea", "Aeroport Otopeni Bucuresti"),
            ("Aeroport Otopeni Bucuresti", "Brasov"),
            ("Chisinau", "Otopeni"),
            ("Cernavoda", "Hilton Garden Inn Bucharest Airport"),
            // The hotel at Otopeni: a run to or from it is a regular leg, even when the name says no town.
            ("Constanta", "Hotel Hilton Garden Inn"),
            ("Hotel Hilton Garden Inn, Aeroport Otopeni", "Agigea"),
            ("Constanta", "Bucuresti Otp ( Hilton Garden Inn)"),
        ] {
            assert!(is_regular_route(from, to), "{from} → {to}");
        }
        // Without the Otopeni end, only Constanța ↔ Bucharest-side pairs of the main route are regular.
        for (from, to) in [
            ("Constanta", "Mangalia"),
            ("Mamaia Nord", "Constanta"),
            ("Navodari", "Cernavoda"),
            ("Tulcea", "Constanta"),
            ("Constanta", "Chisinau"),
            ("Constanta", ""),
        ] {
            assert!(!is_regular_route(from, to), "{from} → {to}");
        }
    }

    #[test]
    fn the_county_is_local_and_the_rest_is_far() {
        let none: &[String] = &[];
        for place in ["Mamaia Nord", "Mamaia", "Eforie Sud", "Vama Veche", "Cernavoda", "Hârșova", "Aeroport Mihail Kogalniceanu", "Constanta", "Mangalia Port", "Negru Vodă", "Aeroport Otopeni Bucuresti"] {
            assert!(is_local(place, none), "{place}");
        }
        for place in ["Tulcea", "Chisinau", "Kalambaka", "Ploiesti", "Poiana Brasov", "Jurilovca", "Chitila", "Brasov"] {
            assert!(!is_local(place, none), "{place}");
        }
        // The office can add what the built-in list misses.
        let extra = vec!["Chitila".to_string()];
        assert!(is_local("Chitila", &extra));
        assert!(!is_local("Tulcea", &extra));
    }

    #[test]
    fn edit_distance() {
        assert_eq!(distance("kogalinceanu", "kogalniceanu"), 2);
        assert_eq!(distance("navodari", "navodari"), 0);
        assert_eq!(distance("", "abc"), 3);
    }
}
