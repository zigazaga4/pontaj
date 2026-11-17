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
    fn edit_distance() {
        assert_eq!(distance("kogalinceanu", "kogalniceanu"), 2);
        assert_eq!(distance("navodari", "navodari"), 0);
        assert_eq!(distance("", "abc"), 3);
    }
}
