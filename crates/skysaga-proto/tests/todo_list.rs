//! The quest log's wire format, against the client's own writers.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::todo_list::{
    ItemObjective, TodoListTaskAdd, TodoListTaskRef, TodoTask, ValueObjective, TASK_LIST_DEFAULT,
};

fn bits(write: impl FnOnce(&mut BitWriter)) -> usize {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.bits_used()
}

/// The one capture, sized exactly.
///
/// `logs/game.log:31599` reads `[recv] … msgId 280 (280) length 9`. Nine bytes is 72 bits;
/// take off the two-byte extended id and 56 bits of body remain. One objective present with a
/// resource and a small count, the other two absent:
///
/// ```text
/// taskId              8
/// flag                1
/// objectiveA  present 1 + optional 1 + 32 + count 1 + 7   = 42
/// objectiveB  absent  1
/// objectiveC  absent  1
/// timedType           2
/// manuallyAdded       1
///                                                    total 56
/// ```
///
/// This is the strongest available confirmation of the layout, and it says what the captured
/// packet *was*: a player putting one item objective on the list.
#[test]
fn the_captured_add_is_fifty_six_bits() {
    let packet = TodoListTaskAdd {
        task: TodoTask {
            task_id: 3,
            flag: false,
            objective_a: Some(ItemObjective {
                resource: Some(0x1234_5678),
                count: 5,
            }),
            objective_b: None,
            objective_c: None,
            timed_challenge_type: 0,
        },
        manually_added: true,
    };

    assert_eq!(bits(|w| packet.encode(w)), 56);
}

#[test]
fn an_add_round_trips() {
    let packet = TodoListTaskAdd {
        task: TodoTask {
            task_id: 200,
            flag: true,
            objective_a: Some(ItemObjective {
                resource: Some(7),
                count: 64,
            }),
            objective_b: Some(ItemObjective {
                resource: None,
                count: 65,
            }),
            objective_c: Some(ValueObjective {
                resource: Some(9),
                value: 0xDEAD_BEEF,
            }),
            timed_challenge_type: 2,
        },
        manually_added: false,
    };

    let mut writer = BitWriter::new();
    packet.encode(&mut writer);

    let bytes = writer.into_bytes();
    let mut reader = BitReader::from_bytes(&bytes);

    assert_eq!(TodoListTaskAdd::decode(&mut reader).unwrap(), packet);
}

/// A count of 64 is still the *small* form; 65 is the first wide one. `FUN_00794dc0` branches
/// on `< 0x41`, so the boundary is inclusive and an off-by-one here shifts everything after.
#[test]
fn the_count_width_switches_above_sixty_four() {
    let with_count = |count| {
        bits(|w| {
            TodoTask {
                objective_a: Some(ItemObjective {
                    resource: None,
                    count,
                }),
                ..Default::default()
            }
            .encode(w)
        })
    };

    // taskId 8 + flag 1 + A(1 + 1 + count) + B 1 + C 1 + type 2
    assert_eq!(with_count(64), 8 + 1 + (1 + 1 + 1 + 7) + 1 + 1 + 2);
    assert_eq!(with_count(65), 8 + 1 + (1 + 1 + 1 + 17) + 1 + 1 + 2);
}

/// Objective C carries a flat word, not the 7/17 form. `FUN_0084b1c0` calls `FUN_00778da0`.
#[test]
fn objective_c_is_a_full_word() {
    let task = TodoTask {
        objective_c: Some(ValueObjective {
            resource: None,
            value: 1,
        }),
        ..Default::default()
    };

    assert_eq!(bits(|w| task.encode(w)), 8 + 1 + 1 + 1 + (1 + 1 + 32) + 2);
}

/// An empty task is the shortest record: three absent objectives is three bits.
#[test]
fn an_empty_task_is_thirteen_bits() {
    assert_eq!(bits(|w| TodoTask::default().encode(w)), 8 + 1 + 3 + 2);
}

/// The three id-only packets are identical but for their ordinal.
#[test]
fn a_task_reference_is_one_byte() {
    let packet = TodoListTaskRef { task_id: 17 };

    assert_eq!(bits(|w| packet.encode(w)), 8);

    let mut writer = BitWriter::new();
    packet.encode(&mut writer);

    let bytes = writer.into_bytes();

    assert_eq!(
        TodoListTaskRef::decode(&mut BitReader::from_bytes(&bytes)).unwrap(),
        packet,
    );

    assert_eq!(
        [
            TodoListTaskRef::ERASE,
            TodoListTaskRef::REMOVE,
            TodoListTaskRef::READD,
        ],
        [145, 147, 148],
    );
}

/// An empty list is its six-bit count and nothing else, which is what the panel needs.
#[test]
fn an_empty_task_list_is_six_bits() {
    assert_eq!(bits(|w| TodoTask::encode_list(&[], w)), 6);
}

/// The list's boundary follows the same rule as every other count-optimised list: at the cap
/// exactly, a clear bit and no 32-bit count.
#[test]
fn the_task_list_boundary_is_a_clear_bit() {
    let task = TodoTask::default();
    let record = 8 + 1 + 3 + 2;

    for (count, header) in [(31, 6), (32, 6 + 1), (33, 6 + 1 + 32)] {
        let tasks = vec![task; count];

        assert_eq!(
            bits(|w| TodoTask::encode_list(&tasks, w)),
            header + count * record,
            "{count} tasks",
        );
    }

    assert_eq!(TASK_LIST_DEFAULT, 32);
}

#[test]
fn a_list_round_trips() {
    let tasks = vec![
        TodoTask {
            task_id: 0,
            flag: true,
            objective_a: Some(ItemObjective {
                resource: Some(11),
                count: 3,
            }),
            ..Default::default()
        },
        TodoTask {
            task_id: 1,
            objective_c: Some(ValueObjective {
                resource: Some(12),
                value: 400,
            }),
            timed_challenge_type: 1,
            ..Default::default()
        },
    ];

    let mut writer = BitWriter::new();
    TodoTask::encode_list(&tasks, &mut writer);

    let bytes = writer.into_bytes();

    assert_eq!(
        TodoTask::decode_list(&mut BitReader::from_bytes(&bytes)).unwrap(),
        tasks,
    );
}
