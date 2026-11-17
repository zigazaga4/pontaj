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

const BUCHAREST: [&str; 5] = ["otopeni", "bucuresti", "baneasa", "banea", "henri coanda"];
const CONSTANTA: [&str; 2] = ["constanta", "mamaia"];

/// Where a place is, as far as "did the driver stay where he arrived" is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Area {
    Constanta,
    Bucuresti,
    Other(String),
}

pub fn area(name: &str) -> Area {
    let normalized = normalize(name);
    if BUCHAREST.iter().any(|k| normalized.contains(k)) {
        Area::Bucuresti
    } else if CONSTANTA.iter().any(|k| normalized.contains(k)) {
        Area::Constanta
    } else {
        Area::Other(normalized)
    }
}

pub fn same_area(a: &str, b: &str) -> bool {
    area(a) == area(b)
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
    "siutghiol", "targusor", "topalu", "topraisar", "tortoman", "tuzla", "valu lui traian", "vama veche", "vadu", "venus", "saturn", "aurora", "vulturu",
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
        assert!(same_area("Aeroport Otopeni Bucuresti", "Bucuresti"));
        assert!(same_area("Aeroport Banea", "Otopeni"));
        assert!(same_area("Mamaia Nord", "Constanta"));
        assert!(!same_area("Constanta", "Cernavoda"));
        assert!(is_main_route("Constanta") && is_main_route("Aeroport Otopeni Bucuresti"));
        assert!(!is_main_route("Mamaia Nord") && !is_main_route("Tulcea") && !is_main_route("Chitila"));
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
