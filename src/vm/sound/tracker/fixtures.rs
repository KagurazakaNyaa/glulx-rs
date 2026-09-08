//! Original minimal tracker files: square wave, mute, and restart after 180 ms.
//! Headers and event bytes are assembled from each public file format.

fn u16_at(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn u32_at(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(super) fn xm() -> Vec<u8> {
    let mut bytes = vec![0; 336];
    bytes[..17].copy_from_slice(b"Extended Module: ");
    bytes[17..30].copy_from_slice(b"Glulx XM test");
    bytes[37] = 0x1a;
    bytes[38..54].copy_from_slice(b"FastTracker v2.0");
    u16_at(&mut bytes, 58, 0x0104);
    u32_at(&mut bytes, 60, 276);
    for (offset, value) in [
        (64, 1),
        (68, 1),
        (70, 1),
        (72, 1),
        (74, 1),
        (76, 3),
        (78, 125),
    ] {
        u16_at(&mut bytes, offset, value);
    }
    // Three uncompressed rows, one channel: note+instrument/F03, C00, B00.
    bytes.extend_from_slice(&[9, 0, 0, 0, 0, 3, 0, 15, 0]);
    bytes.extend_from_slice(&[49, 1, 0, 15, 3, 0, 0, 0, 12, 0, 0, 0, 0, 11, 0]);
    let mut instrument = vec![0; 263];
    u32_at(&mut instrument, 0, 263);
    u16_at(&mut instrument, 27, 1);
    u32_at(&mut instrument, 29, 40);
    bytes.extend(instrument);
    let mut sample = vec![0; 40];
    u32_at(&mut sample, 0, 64);
    u32_at(&mut sample, 8, 64);
    sample[12] = 64;
    sample[14] = 1;
    sample[15] = 128;
    bytes.extend(sample);
    let mut waveform = [0u8; 64];
    waveform[0] = 96;
    waveform[32] = 64; // Delta -192 wraps to64; decoded amplitude becomes-96.
    bytes.extend(waveform);
    bytes
}

pub(super) fn s3m() -> Vec<u8> {
    let mut bytes = vec![0; 832];
    bytes[..14].copy_from_slice(b"Glulx S3M test");
    bytes[28] = 0x1a;
    bytes[29] = 0x10;
    for (offset, value) in [(32, 1), (34, 1), (36, 1), (40, 0x1320), (42, 2)] {
        u16_at(&mut bytes, offset, value);
    }
    bytes[44..48].copy_from_slice(b"SCRM");
    bytes[48..52].copy_from_slice(&[64, 3, 125, 0xc0]);
    bytes[64..96].fill(255);
    bytes[64] = 0;
    u16_at(&mut bytes, 97, 0x10); // Instrument at256.
    u16_at(&mut bytes, 99, 0x20); // Pattern at512.
    bytes[256] = 1;
    bytes[270] = 0x30; // Sample at768, paragraph pointer low byte.
    u32_at(&mut bytes, 272, 64);
    u32_at(&mut bytes, 280, 64);
    bytes[284] = 64;
    bytes[287] = 1;
    u32_at(&mut bytes, 288, 8363);
    bytes[332..336].copy_from_slice(b"SCRS");
    let mut pattern = vec![0xa0, 0x40, 1, 1, 3, 0, 0x40, 0, 0, 0x80, 2, 0, 0];
    pattern.extend([0; 61]);
    u16_at(&mut bytes, 512, pattern.len() as u16);
    bytes[514..514 + pattern.len()].copy_from_slice(&pattern);
    bytes[768..800].fill(224);
    bytes[800..832].fill(32);
    bytes
}

pub(super) fn it() -> Vec<u8> {
    let mut bytes = vec![0; 576];
    bytes[..4].copy_from_slice(b"IMPM");
    bytes[4..17].copy_from_slice(b"Glulx IT test");
    bytes[30..32].copy_from_slice(&[4, 16]);
    for (offset, value) in [
        (32, 2),
        (36, 1),
        (38, 1),
        (40, 0x0214),
        (42, 0x0214),
        (44, 1),
    ] {
        u16_at(&mut bytes, offset, value);
    }
    bytes[48..53].copy_from_slice(&[128, 48, 3, 125, 128]);
    bytes[64..128].fill(0xa0);
    bytes[64] = 32;
    bytes[128..192].fill(64);
    bytes[193] = 255;
    u32_at(&mut bytes, 194, 256);
    u32_at(&mut bytes, 198, 384);
    bytes[256..260].copy_from_slice(b"IMPS");
    bytes[273] = 64;
    bytes[274] = 17; // Present+forwardloop.
    bytes[275] = 64;
    bytes[302] = 1; // Signed8bitPCM.
    bytes[303] = 160; // Enabledcenterpan.
    u32_at(&mut bytes, 304, 64);
    u32_at(&mut bytes, 312, 64);
    u32_at(&mut bytes, 316, 8363);
    u32_at(&mut bytes, 328, 512);
    let mut pattern = vec![
        0x81, 0x0b, 60, 1, 1, 3, 0, 0x81, 0x04, 0, 0, 0x81, 0x08, 2, 0, 0,
    ];
    pattern.extend([0; 61]);
    u16_at(&mut bytes, 384, pattern.len() as u16);
    u16_at(&mut bytes, 386, 64);
    bytes[392..392 + pattern.len()].copy_from_slice(&pattern);
    bytes[512..544].fill(96);
    bytes[544..576].fill(160);
    bytes
}

pub(super) fn s3m_adlib() -> Vec<u8> {
    let mut bytes = s3m();
    bytes[64] = 16; // First AdLib melody channel.
    bytes[256] = 2; // Two-operator OPL instrument.
    bytes[269..284].fill(0);
    // Modulator/carrier: sustain, multiplier1; fast attack, slow release;
    // additive connection so both sine operators contribute to the note.
    bytes[272..284].copy_from_slice(&[0x21, 0x21, 0x10, 0, 0xf0, 0xf0, 0x03, 0x03, 0, 0, 1, 0]);
    bytes[287] = 0;
    bytes[332..336].copy_from_slice(b"SCRI");
    bytes
}
