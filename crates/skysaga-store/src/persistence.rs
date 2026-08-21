//! Carrying changes from the synchronous state into the asynchronous store.
//!
//! [`AppState`](skysaga_state::AppState) is synchronous and is read from two very different
//! places: async web handlers, and the game server's own OS thread. Making it async would
//! push `await` into both and turn every state read on the game thread into a runtime call.
//!
//! So it stays synchronous and reports changes to a sink. This is that sink: it queues on an
//! unbounded channel, which never blocks the caller, and a background task drains the queue
//! into the database.
//!
//! # What that costs
//!
//! Writes are not durable at the moment the state changes; they are durable a moment later.
//! A crash in that window loses the last few changes. That is the right trade here, because
//! the alternative is a database round trip inside the game loop, and the data at stake is a
//! character's name and appearance rather than anything a player pays for.
//!
//! Ordering *is* guaranteed. The channel is FIFO and the task applies changes one at a time,
//! so "create character" cannot overtake "delete character".

use std::sync::Arc;

use skysaga_state::{Change, ChangeSink};
use tokio::sync::mpsc::{self, UnboundedSender};
use tracing::{error, warn};

use crate::Store;

/// A [`ChangeSink`] that writes to a [`Store`] in the background.
pub struct Persistence {
    changes: UnboundedSender<Change>,
}

impl Persistence {
    /// Start the background writer.
    ///
    /// Must be called from inside a tokio runtime. The task lives as long as the sink does:
    /// dropping every `Persistence` closes the channel and the task finishes its queue and
    /// stops.
    pub fn start(store: Arc<dyn Store>) -> Self {
        let (changes, mut queue) = mpsc::unbounded_channel::<Change>();

        tokio::spawn(async move {
            while let Some(change) = queue.recv().await {
                if let Err(error) = apply(store.as_ref(), &change).await {
                    // A failed write must not take the server down: the player is mid-session
                    // and the in-memory state is still correct. It is logged loudly because
                    // it means this change will not survive a restart.
                    error!(%error, ?change, "could not persist a change");
                }
            }
        });

        Self { changes }
    }
}

impl ChangeSink for Persistence {
    fn record(&self, change: Change) {
        // Fails only once the receiver is gone, which means the writer task has stopped.
        if self.changes.send(change).is_err() {
            warn!("the persistence task has stopped; this change is not being written");
        }
    }
}

async fn apply(store: &dyn Store, change: &Change) -> Result<(), crate::StoreError> {
    match change {
        Change::Account { key, display_name } => store.save_account(key, display_name).await,

        Change::Character { account, character } => store.save_character(account, character).await,

        Change::DeleteCharacter { account } => store.delete_character(account).await,

        Change::Photo { id, photo } => store.save_photo(id, photo).await,
    }
}
