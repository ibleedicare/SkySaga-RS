//! Which recipe or resource does a name hash name? A one-off for reading a server log.

fn main() {
    let target: u32 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .expect("a hash");

    let geo = skysaga_world::geodata::GeoData::load(skysaga_world::geodata::default_geodata_path())
        .expect("geodata.json");

    for recipe in geo.recipes() {
        if recipe.id() == target {
            println!(
                "recipe {} station {:?} -> {:?}",
                recipe.name,
                recipe.station(),
                recipe.output()
            );
        }
    }
}
