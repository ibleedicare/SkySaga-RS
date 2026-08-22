//! The mailbox packets.
//!
//! Seven of them, and between them they carry two strings and nothing else. All the mail data
//! travels the other way, as `Player.mailitemlist` inside an ordinary entity sync -- so these
//! are requests and acknowledgements, not the mail itself.
//!
//! Ids were proved individually from the client's send primitive `FUN_00896290(id, ...)`
//! rather than assumed from the enum's order; see `documentations/mail.md` §1.

use skysaga_proto::bitstream::{BitReader, BitWriter};
use skysaga_proto::packets::mail::{
    DeleteMail, MailCheck, MailGiftSelected, MailRead, NewMailReceived, RemoteMailSynced,
    TakeMailAttachment,
};

fn encoded(write: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut writer = BitWriter::new();

    write(&mut writer);

    writer.into_bytes()
}

fn reader(bytes: &[u8], expect_id: u16) -> BitReader<'_> {
    let mut reader = BitReader::from_bytes(bytes);

    assert_eq!(reader.read_packet_id().unwrap(), expect_id);

    reader
}

#[test]
fn the_ids_are_the_ones_proved_from_the_client() {
    // Not derived from the enum's order: each was read off the immediate pushed to the send
    // primitive at a named address. Pinning them here means a renumbering has to be a decision.
    assert_eq!(NewMailReceived::ID, 93);
    assert_eq!(MailRead::ID, 94);
    assert_eq!(MailGiftSelected::ID, 95);
    assert_eq!(MailCheck::ID, 96);
    assert_eq!(DeleteMail::ID, 97);
    assert_eq!(TakeMailAttachment::ID, 98);
    assert_eq!(RemoteMailSynced::ID, 99);
}

#[test]
fn a_mail_check_is_one_byte() {
    // The client writes no fields at all, so the id is the whole message.
    let bytes = encoded(|w| MailCheck.encode(w));

    assert_eq!(bytes.len(), 1);
    assert_eq!(MailCheck::decode(&mut reader(&bytes, 96)).unwrap(), MailCheck);
}

#[test]
fn a_remote_mail_synced_is_one_byte_too() {
    // The packet that stops the panel saying "loading". Its deserialiser reads nothing and
    // just fires the completion callback.
    let bytes = encoded(|w| RemoteMailSynced.encode(w));

    assert_eq!(bytes.len(), 1);
}

#[test]
fn the_single_string_packets_round_trip() {
    let uuid = "78cea81f-0000-4000-8000-000000000001";

    let read = MailRead {
        message_uuid: uuid.to_owned(),
    };

    let bytes = encoded(|w| read.encode(w));
    assert_eq!(MailRead::decode(&mut reader(&bytes, 94)).unwrap(), read);

    let gift = MailGiftSelected {
        message_uuid: uuid.to_owned(),
    };

    let bytes = encoded(|w| gift.encode(w));
    assert_eq!(
        MailGiftSelected::decode(&mut reader(&bytes, 95)).unwrap(),
        gift,
    );

    let delete = DeleteMail {
        message_uuid: uuid.to_owned(),
    };

    let bytes = encoded(|w| delete.encode(w));
    assert_eq!(DeleteMail::decode(&mut reader(&bytes, 97)).unwrap(), delete);
}

#[test]
fn taking_an_attachment_names_both_the_message_and_the_item() {
    // Two strings, in that order. Reading them the other way round claims the right message's
    // attachment only when the two uuids happen to be interchangeable, which is never.
    let packet = TakeMailAttachment {
        message_uuid: "message-uuid".to_owned(),
        item_uuid: "item-uuid".to_owned(),
    };

    let bytes = encoded(|w| packet.encode(w));

    let mut r = reader(&bytes, 98);
    assert_eq!(r.read_string().unwrap(), "message-uuid");
    assert_eq!(r.read_string().unwrap(), "item-uuid");

    assert_eq!(
        TakeMailAttachment::decode(&mut reader(&bytes, 98)).unwrap(),
        packet,
    );
}

#[test]
fn the_doorbell_carries_a_uuid_the_client_throws_away() {
    // Its handler does nothing but send `MailCheck` straight back. The string is still read
    // off the stream, so it has to be present and well formed -- an absent one desynchronises
    // the reader rather than being ignored.
    let packet = NewMailReceived {
        message_uuid: "78cea81f-0000-4000-8000-000000000001".to_owned(),
    };

    let bytes = encoded(|w| packet.encode(w));

    let mut r = reader(&bytes, 93);

    assert_eq!(r.read_string().unwrap(), packet.message_uuid);
}

#[test]
fn a_truncated_mail_packet_is_an_error_rather_than_a_panic() {
    for length in 0..4 {
        let bytes = vec![0u8; length];

        let mut r = BitReader::from_bytes(&bytes);
        let _ = r.read_packet_id();

        let _ = MailRead::decode(&mut r);
        let _ = TakeMailAttachment::decode(&mut r);
    }
}
