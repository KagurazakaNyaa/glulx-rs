//! Original SONG/MOD and AIFF instruments, generated from numeric waveforms.

pub(super) fn song(references: &[(&str, u8, u8)]) -> Vec<u8> {
    let mut song = vec![0; 1084 + 1024];
    song[..15].copy_from_slice(b"Glulx SONG test");
    song[950] = 1;
    song[1080..1084].copy_from_slice(b"M.K.");
    for (index, (reference, finetune, volume)) in references.iter().enumerate() {
        let start = 20 + index * 30;
        song[start..start + reference.len()].copy_from_slice(reference.as_bytes());
        // Deliberately impossible legacy lengths/loops must be ignored.
        song[start + 22..start + 24].fill(0xff);
        song[start + 24] = *finetune;
        song[start + 25] = *volume;
        song[start + 26..start + 30].fill(0xff);
    }
    song[1084..1088].copy_from_slice(&[0x01, 0xac, 0x1f, 0x03]);
    song[1100..1104].copy_from_slice(&[0, 0, 0x0c, 0]);
    song[1116..1120].copy_from_slice(&[0, 0, 0x0b, 0]);
    song
}

pub(super) fn module(pcm: &[i8], start: u16, length: u16, finetune: u8, volume: u8) -> Vec<u8> {
    let mut module = song(&[("", finetune, volume)]);
    module[42..44].copy_from_slice(&(pcm.len() as u16 / 2).to_be_bytes());
    module[46..48].copy_from_slice(&(start / 2).to_be_bytes());
    module[48..50].copy_from_slice(&(length / 2).to_be_bytes());
    module.extend(pcm.iter().map(|sample| *sample as u8));
    module
}

pub(super) fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
    output.extend_from_slice(kind);
    output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    output.extend_from_slice(bytes);
    if !bytes.len().is_multiple_of(2) {
        output.push(0);
    }
}

// Points use signed Q1.31. AIFF packs the requested high bits, left justified.
pub(super) fn aiff(
    bits: u16,
    channels: u16,
    points: &[i32],
    looping: Option<(u16, u32, u32)>,
    offset: u32,
) -> Vec<u8> {
    let mut common = channels.to_be_bytes().to_vec();
    common.extend_from_slice(&(points.len() as u32 / u32::from(channels)).to_be_bytes());
    common.extend_from_slice(&bits.to_be_bytes());
    common.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]); // 44100 Hz.
    let mut sound = offset.to_be_bytes().to_vec();
    sound.extend_from_slice(&0u32.to_be_bytes());
    sound.extend(std::iter::repeat_n(0x55, offset as usize));
    for point in points {
        let value = *point & (u32::MAX << (32 - bits)) as i32;
        sound.extend_from_slice(&value.to_be_bytes()[..usize::from(bits.div_ceil(8))]);
    }
    let mut bytes = b"FORM\0\0\0\0AIFF".to_vec();
    chunk(&mut bytes, b"ANNO", b"odd");
    chunk(&mut bytes, b"SSND", &sound);
    if let Some((mode, start, end)) = looping {
        let mut markers = 2u16.to_be_bytes().to_vec();
        for (id, position, name) in [(300u16, start, "begin"), (701, end, "end!")] {
            markers.extend_from_slice(&id.to_be_bytes());
            markers.extend_from_slice(&position.to_be_bytes());
            markers.push(name.len() as u8);
            markers.extend_from_slice(name.as_bytes());
            if name.len().is_multiple_of(2) {
                markers.push(0);
            }
        }
        chunk(&mut bytes, b"MARK", &markers);
        let mut instrument = vec![60, 0, 0, 127, 1, 127, 0, 0];
        for value in [mode, 300, 701, 1, 300, 701] {
            instrument.extend_from_slice(&value.to_be_bytes());
        }
        chunk(&mut bytes, b"INST", &instrument);
    }
    chunk(&mut bytes, b"COMM", &common);
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_be_bytes());
    bytes
}

pub(super) fn story(song: &[u8], instrument: &[u8]) -> crate::Story {
    let image = crate::vm::tests::image_with_program(&[0x81, 0x20]);
    let chunks = [
        (*b"Exec", 0u32, *b"GLUL", image.as_slice()),
        (*b"Snd ", 7, *b"SONG", song),
        (*b"Snd ", 42, *b"FORM", &instrument[8..]),
    ];
    let mut index = (chunks.len() as u32).to_be_bytes().to_vec();
    let mut offset = 12 + 8 + 4 + 12 * chunks.len();
    for (usage, number, _, data) in &chunks {
        index.extend_from_slice(usage);
        index.extend_from_slice(&number.to_be_bytes());
        index.extend_from_slice(&(offset as u32).to_be_bytes());
        offset += 8 + data.len() + data.len() % 2;
    }
    let mut bytes = b"FORM\0\0\0\0IFRS".to_vec();
    chunk(&mut bytes, b"RIdx", &index);
    for (_, _, kind, data) in chunks {
        chunk(&mut bytes, &kind, data);
    }
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_be_bytes());
    crate::Story::from_bytes(&bytes, None).unwrap()
}
