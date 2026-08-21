//! Does any entity parameter get bound by more than one component?
//!
//! If so, which component owns a sync index depends on iteration order, and the loader must
//! pick deterministically rather than by whichever the map yielded first.

use std::collections::BTreeMap;

fn main() {
    let text = std::fs::read_to_string(skysaga_world::default_entities_path()).unwrap();
    let file: serde_json::Value = serde_json::from_str(&text).unwrap();

    let mut total = 0;
    let mut collisions = 0;

    for entity in file["Entities"].as_array().unwrap() {
        let name = entity["Name"].as_str().unwrap_or("?");

        // entity parameter -> components binding it
        let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();

        let Some(components) = entity["client"]["components"].as_object() else {
            continue;
        };

        for (component, body) in components {
            let Some(bindings) = body["bindings"].as_object() else {
                continue;
            };

            for (_binding, target) in bindings {
                if let Some(mapsto) = target["mapsto"].as_str() {
                    owners
                        .entry(mapsto.to_owned())
                        .or_default()
                        .push(component.clone());
                }
            }
        }

        for (parameter, mut components) in owners {
            components.sort();
            components.dedup();

            // Only a *synced* parameter matters: an unsynced one has no index to fight over.
            let synced = entity["parameters"][&parameter]["syncindex"].is_number();

            total += usize::from(synced);

            if synced && components.len() > 1 {
                collisions += 1;

                println!("{name}: {parameter} bound by {components:?}");
            }
        }
    }

    println!("\n{collisions} collisions out of {total} synced bindings");
}
