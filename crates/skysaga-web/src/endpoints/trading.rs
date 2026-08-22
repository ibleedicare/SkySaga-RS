//! The Trading_Post's two endpoints.
//!
//! Both are **HTTP, not RakNet**. The client's string table holds `/api/trading/find` and
//! `/api/trading/catalogue`, and no trading packet exists at all. The panel posts to `find`
//! on its search tab and to `catalogue` on the others; the listings are server-authored
//! either way, so both answer the same thing.
//!
//! # A second player is never needed to exercise this
//!
//! Every listing is JSON this server invents. Other players' offers are not read from anywhere
//! -- there is no market, no stock and no economy behind them. That is what makes the trading
//! post testable at all with one client.
//!
//! # The response shape has no wrapper key, and gets no error when it is wrong
//!
//! The client's parser walks the **direct children** of `result` and searches each for seven
//! keys. All seven must be present or the entry is dropped through a single `&&` gate, with
//! nothing logged: the row simply never appears. So a wrapper such as `result.listings`, or
//! one missing key, gives an empty tab and a silent server.
//!
//! | key | type | |
//! |---|---|---|
//! | `uuid` | string | the listing id |
//! | `type` | string | the resource **name**; the client hashes it and looks it up |
//! | `numberAvailable` | number | |
//! | `costPerUnit` | number | |
//! | `seller` | object | `characterUuid`, `characterName`, `homeworldKey` |
//! | `world` | string | |
//! | `itemSpec` | object | `res`, `mat1`..`mat4`, `teachItem`, all numeric hashes |
//!
//! # Buying is a teleport, and is not implemented
//!
//! Each listing carries a `world` and a `seller.homeworldKey`, and the UI has a
//! `ui_trade_visit` element: clicking a trade travels to the seller's home island rather than
//! transferring an item in place. That makes the purchase path a client of the teleport work,
//! not of this file. Browsing is what is done here.

use axum::routing::post;
use axum::{Json, Router};
use serde::Serialize;
use tracing::debug;

use crate::Api;

pub fn router() -> Router<Api> {
    Router::new()
        .route("/api/trading/catalogue", post(catalogue))
        .route("/api/trading/find", post(catalogue))
}

/// The envelope: `result` **is** the list. See the module docs on why there is no key here.
#[derive(Debug, Serialize)]
struct Listings {
    result: Vec<Listing>,
}

#[derive(Debug, Serialize)]
struct Listing {
    uuid: String,
    /// The resource **name**, not its hash: the client hashes this itself before looking it
    /// up, so a hash here would be hashed again and resolve to nothing.
    #[serde(rename = "type")]
    item: String,
    #[serde(rename = "numberAvailable")]
    number_available: u32,
    #[serde(rename = "costPerUnit")]
    cost_per_unit: u32,
    seller: Seller,
    world: String,
    #[serde(rename = "itemSpec")]
    item_spec: ItemSpec,
}

#[derive(Debug, Serialize)]
struct Seller {
    #[serde(rename = "characterUuid")]
    character_uuid: String,
    #[serde(rename = "characterName")]
    character_name: String,
    #[serde(rename = "homeworldKey")]
    homeworld_key: String,
}

#[derive(Debug, Serialize)]
struct ItemSpec {
    /// The hash of the listing's `type`.
    res: u32,
    mat1: u32,
    mat2: u32,
    mat3: u32,
    mat4: u32,
    #[serde(rename = "teachItem")]
    teach_item: u32,
}

/// Invented listings from invented sellers.
///
/// One per category tab, so no tab comes up empty. The panel's five tabs filter this same
/// catalogue by each resource's own category, so a blank tab means nothing listed falls into
/// it -- the filter working, not a fault.
const CATALOGUE: &[(&str, &str, u32, u32, &str)] = &[
    // uuid,                                   item,                 stock, price, seller
    ("11111111-1111-4111-8111-111111111111", "Dirt", 64, 1, "Bramblewick"),
    ("22222222-2222-4222-8222-222222222222", "Metal_Battleaxe", 1, 250, "Hob"),
    ("33333333-3333-4333-8333-333333333333", "GuardianArmourHead", 2, 500, "Marrow"),
    ("44444444-4444-4444-8444-444444444444", "Animal_Meat", 12, 15, "Pellin"),
    ("55555555-5555-4555-8555-555555555555", "Mushroom", 30, 4, "Osgood"),
];

async fn catalogue(body: String) -> Json<impl Serialize> {
    debug!(%body, "trading catalogue requested");

    Json(Listings {
        result: CATALOGUE
            .iter()
            .map(|(uuid, item, stock, price, seller)| Listing {
                uuid: (*uuid).to_owned(),
                item: (*item).to_owned(),
                number_available: *stock,
                cost_per_unit: *price,
                seller: Seller {
                    character_uuid: (*uuid).to_owned(),
                    character_name: (*seller).to_owned(),
                    homeworld_key: "home".to_owned(),
                },
                world: "home".to_owned(),
                item_spec: ItemSpec {
                    res: skysaga_core::name_hash(item),
                    mat1: 0,
                    mat2: 0,
                    mat3: 0,
                    mat4: 0,
                    teach_item: 0,
                },
            })
            .collect(),
    })
}
