//! `TodoListComponent` — the quest log's contents.
//!
//! | parameter | bits | |
//! |---|---:|---|
//! | `tasklist` | 6 + a record each | count-optimised, default 32 |
//!
//! One parameter, bound on `Player` as `todotasklist` at sync index 82. The component is
//! declared on exactly three entities — `Player`, `ArtTestPlayer`, `TestPlayer` — which is the
//! same set that carries `clientjobrankcomponent` and `clientfeatureunlockcomponent`.
//!
//! # Why an empty list still has to go out
//!
//! The quest log panel renders from this parameter. An *absent* parameter is not an empty log:
//! the client has no list object at all, so the panel draws its frame and then has nothing to
//! attach rows or click targets to. Sending a zero-length list is six bits and is what makes
//! the panel live.
//!
//! # The record is the packet
//!
//! A row here and the body of `TodoListTaskAdd` are the same 48-byte record, written by the
//! same primitives in the same order (`FUN_008a9e30` against `FUN_0084b570`). It lives in
//! `skysaga_proto::packets::todo_list` so the component and the handler cannot drift apart.

use skysaga_proto::bitstream::BitWriter;
use skysaga_proto::packets::todo_list::TodoTask;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TodoListComponent {
    pub tasks: Vec<TodoTask>,
}

impl TodoListComponent {
    pub fn sync(&self, parameter: &str, writer: &mut BitWriter) -> bool {
        match parameter.to_ascii_lowercase().as_str() {
            // The binding is `todotasklist` on the entity and `tasklist` on the component;
            // the sync map dispatches on the component's name for it. Both are accepted
            // because the two spellings are one field and a mismatch is silent.
            "tasklist" | "todotasklist" => TodoTask::encode_list(&self.tasks, writer),

            _ => return false,
        }

        true
    }

    /// The lowest task id not in use.
    ///
    /// Ids are 8 bits and the list caps at 32, so the server allocates them; they are not
    /// hashes. The client's UI element is `Todo_00_tutorial`, i.e. slot-indexed, so ids are
    /// packed from zero rather than ever-increasing.
    pub fn free_id(&self) -> Option<u8> {
        (0..=u8::MAX).find(|candidate| !self.tasks.iter().any(|task| task.task_id == *candidate))
    }
}
