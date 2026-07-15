//! Human-readable display labels for edition names — shared between the
//! games-list/summary rows and the custom-game builder's picker, both of
//! which only ever need to cover `rules_shared::VariantRules::EDITION_NAMES`.

pub fn edition_label(name: &str) -> &'static str {
    match name {
        "official" => "English (International)",
        "wordfeud" => "English (Wordfeud)",
        "north_american" => "English (Americas)",
        "german" => "German",
        "spanish" => "Spanish (Castilian)",
        _ => "Unknown edition",
    }
}
