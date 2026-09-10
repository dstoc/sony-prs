use prsctl::protocol::{Answer, ReadCommand, ANSWER_HEADER_LEN};

#[test]
fn answer_fixture_round_trips() {
    let mut packet = vec![0u8; ANSWER_HEADER_LEN];
    packet[12..16].copy_from_slice(&3u32.to_le_bytes());
    packet.extend_from_slice(b"PRS");
    let answer = Answer::decode(&packet).unwrap();
    assert_eq!(answer.data, b"PRS");
}

#[test]
fn read_command_ids_do_not_include_mutating_commands() {
    let commands = [
        ReadCommand::FileOpen,
        ReadCommand::FileClose,
        ReadCommand::GetSize,
        ReadCommand::SetPosition,
        ReadCommand::GetPosition,
        ReadCommand::FileRead,
        ReadCommand::DirectoryIteratorNew,
        ReadCommand::DirectoryIteratorDispose,
        ReadCommand::DirectoryIteratorGetNext,
        ReadCommand::GetProperty,
        ReadCommand::GetMediaInfo,
        ReadCommand::GetFreeSpace,
    ];
    let ids: Vec<u32> = commands.iter().map(|command| *command as u32).collect();
    for forbidden in [
        0x13, 0x17, 0x1a, 0x1b, 0x1c, 0x30, 0x31, 0x32, 0x300, 0x301, 0x302, 0x303,
    ] {
        assert!(!ids.contains(&forbidden));
    }
}
