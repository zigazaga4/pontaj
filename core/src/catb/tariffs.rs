//! The price list for loadings/unloadings outside the Constanța–Otopeni route ("DESCĂRCĂRI / ÎNCĂRCĂRI EXTERIOARE").

use serde::{Deserialize, Serialize};

use super::places::{distance, normalize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tariff {
    pub name: String,
    pub amount: f64,
}

/// The printed list of the firm, with the handwritten additions (Hârșova 90, Tulcea 110, Jurilovca 120 per trip…).
pub fn default_tariffs() -> Vec<Tariff> {
    const LIST: [(&str, f64); 36] = [
        ("23 August", 40.0),
        ("2 Mai", 40.0),
        ("Agigea", 20.0),
        ("Albești", 40.0),
        ("Amzacea", 30.0),
        ("Comana", 40.0),
        ("Constanța", 20.0),
        ("Corbu", 30.0),
        ("Costinești", 40.0),
        ("Cumpăna", 20.0),
        ("Eforie Nord", 30.0),
        ("Eforie Sud", 30.0),
        ("Ghindărești", 50.0),
        ("Hârșova", 90.0),
        ("Jupiter", 40.0),
        ("Jurilovca", 120.0),
        ("Lazu", 20.0),
        ("Limanu", 40.0),
        ("Lumina", 30.0),
        ("Mamaia Nord", 20.0),
        ("Mangalia", 40.0),
        ("Mihail Kogălniceanu", 30.0),
        ("Mosneni", 40.0),
        ("Nicolae Bălcescu", 40.0),
        ("Năvodari", 30.0),
        ("Negru Vodă", 50.0),
        ("Neptun", 40.0),
        ("Ovidiu", 20.0),
        ("Pecineaga", 40.0),
        ("Ploiești", 50.0),
        ("Poiana", 20.0),
        ("Techirghiol", 30.0),
        ("Topraisar", 30.0),
        ("Tulcea", 110.0),
        ("Tuzla", 30.0),
        ("Vama Veche", 40.0),
    ];
    LIST.iter().map(|(name, amount)| Tariff { name: (*name).to_string(), amount: *amount }).collect()
}

/// The tariff of a place named in the API: the same name (diacritics and case aside), a name that contains it
/// ("Aeroport Mihail Kogalinceanu"), or a close spelling (one or two letters off in a long name).
pub fn find<'a>(tariffs: &'a [Tariff], place: &str) -> Option<&'a Tariff> {
    let words: Vec<String> = normalize(place).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect();
    if words.is_empty() {
        return None;
    }
    let prepared: Vec<(&Tariff, Vec<String>)> =
        tariffs.iter().map(|t| (t, normalize(&t.name).split(' ').filter(|w| !w.is_empty()).map(str::to_string).collect())).collect();

    // 1. The same words, anywhere in the place name; the longest tariff name wins ("Eforie Nord" over "Eforie").
    let contained = prepared
        .iter()
        .filter(|(_, name)| !name.is_empty() && words.windows(name.len()).any(|window| window == name.as_slice()))
        .max_by_key(|(_, name)| name.join(" ").len());
    if let Some((tariff, _)) = contained {
        return Some(tariff);
    }

    // 2. A close spelling, over windows with as many words as the tariff name.
    prepared
        .iter()
        .filter(|(_, name)| !name.is_empty())
        .find(|(_, name)| {
            let wanted = name.join(" ");
            let allowed = match wanted.len() {
                0..=5 => 0,
                6..=7 => 1,
                _ => 2,
            };
            allowed > 0 && words.windows(name.len()).any(|window| distance(&window.join(" "), &wanted) <= allowed)
        })
        .map(|(tariff, _)| *tariff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amount(place: &str) -> Option<f64> {
        find(&default_tariffs(), place).map(|t| t.amount)
    }

    #[test]
    fn matches_names_as_the_api_writes_them() {
        assert_eq!(amount("Navodari"), Some(30.0));
        assert_eq!(amount("Tulcea"), Some(110.0));
        assert_eq!(amount("Hârșova"), Some(90.0));
        assert_eq!(amount("Eforie Nord"), Some(30.0));
        assert_eq!(amount("Aeroport Mihail Kogalinceanu"), Some(30.0), "a misspelled name in the API still finds its tariff");
        assert_eq!(amount("Mamaia Nord"), Some(20.0));
    }

    #[test]
    fn places_without_a_tariff_stay_unmatched() {
        for place in ["Kalambaka", "Chitila", "Chisinau", "Mamaia", "Cernavoda", ""] {
            assert_eq!(amount(place), None, "{place}");
        }
    }

    #[test]
    fn short_names_need_an_exact_word() {
        // Names of up to 5 letters must match exactly: "Lazar" is not "Lazu".
        assert_eq!(amount("Lazar"), None);
        assert_eq!(amount("Lazu"), Some(20.0));
    }
}
