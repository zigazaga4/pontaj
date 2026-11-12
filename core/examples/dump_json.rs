//! Dev helper: prints `{ settings, sheet }` as JSON for UI previews.
fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let settings = pontaj_core::Settings::default();
    let sheet = pontaj_core::process_pdfs(&paths, &settings).expect("parse");
    println!("{}", serde_json::json!({ "settings": settings, "sheet": sheet }));
}
