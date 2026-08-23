//! What every killable creature drops, and which ones drop nothing.
//!
//! ```bash
//! cargo run -p skysaga-world --example loot-coverage
//! ```
//!
//! The mapping from an entity to its table is a naming convention plus a short alias list, so
//! "did that creature get a table" is worth being able to see rather than infer. The drops
//! shown are one roll at a fixed seed: entries under 100% may or may not appear.

use skysaga_world::definitions::default_entities_path;
use skysaga_world::geodata::{default_geodata_path, GeoData};
use skysaga_world::loot::Seeded;
use skysaga_world::EntityDefinitions;

fn main() {
    let Ok(geodata) = GeoData::load(default_geodata_path()) else {
        eprintln!("no geodata.json; set SKYSAGA_GEODATA or SKYSAGA_DATA_DIR");

        return;
    };

    let Ok(definitions) = EntityDefinitions::load(default_entities_path()) else {
        eprintln!("no Entities.json; set SKYSAGA_DATA_DIR");

        return;
    };

    let mut with = Vec::new();
    let mut without = Vec::new();

    for definition in definitions.iter() {
        // Only things that can be killed: no physical properties means no health, and no
        // health means nothing to drop loot from.
        let Some(properties) = definition.physical_properties() else {
            continue;
        };

        if geodata.health_for(properties).unwrap_or(0) == 0 {
            continue;
        }

        match geodata.loot_table_name(definition.name()) {
            Some(table) => {
                // A fixed seed, so two runs agree and a diff means the data changed.
                let mut roll = Seeded::new(7);

                let dropped: Vec<String> = geodata
                    .loot_for(definition.name(), &mut roll)
                    .into_iter()
                    .map(|(item, count)| format!("{item} x{count}"))
                    .collect();

                with.push(format!(
                    "  {:<24} {:<32} {}",
                    definition.name(),
                    table,
                    dropped.join(", "),
                ));
            }

            None => without.push(definition.name().to_owned()),
        }
    }

    with.sort();
    without.sort();

    println!("with loot ({}):", with.len());

    for line in &with {
        println!("{line}");
    }

    println!("\nwithout ({}): {}", without.len(), without.join(", "));
}
