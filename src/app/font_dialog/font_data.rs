//! Read bounded, complete font data through the Windows GDI table interface.

const COLLECTION_TAG: u32 = u32::from_le_bytes(*b"ttcf");
const GDI_ERROR: u32 = u32::MAX;
const MAX_FONT_BYTES: u32 = 64 * 1024 * 1024;

pub(super) fn read_font_data(
    mut read: impl FnMut(u32, Option<&mut [u8]>) -> u32,
) -> Result<Vec<u8>, String> {
    // Zero starts at the selected face inside a TTC, but the face's table
    // offsets are relative to the collection. Request the complete file first.
    // https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-getfontdata
    let mut table = COLLECTION_TAG;
    let mut length = read(table, None);
    if length == GDI_ERROR {
        table = 0;
        length = read(table, None);
    }
    if length == 0 || length > MAX_FONT_BYTES {
        return Err("System font outline data is unavailable or too large".into());
    }
    let mut bytes = vec![0; length as usize];
    if read(table, Some(&mut bytes)) != length {
        return Err("Could not read the system font".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use ab_glyph::{Font, FontRef};
    use eframe::egui;

    use super::*;

    fn copy_font_data(bytes: &[u8], buffer: Option<&mut [u8]>) -> u32 {
        if let Some(buffer) = buffer {
            let length = buffer.len().min(bytes.len());
            buffer[..length].copy_from_slice(&bytes[..length]);
            length as u32
        } else {
            bytes.len() as u32
        }
    }

    fn font_collection(fonts: &[&[u8]]) -> (Vec<u8>, Vec<usize>) {
        let mut bytes = b"ttcf\0\x01\0\0".to_vec();
        bytes.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
        bytes.resize(12 + 4 * fonts.len(), 0);
        let mut offsets = Vec::new();
        for (index, font) in fonts.iter().enumerate() {
            bytes.resize(bytes.len().next_multiple_of(4), 0);
            let offset = bytes.len();
            offsets.push(offset);
            bytes[12 + 4 * index..16 + 4 * index].copy_from_slice(&(offset as u32).to_be_bytes());
            bytes.extend_from_slice(font);
            let tables = u16::from_be_bytes(font[4..6].try_into().unwrap()) as usize;
            for table in 0..tables {
                let position = 12 + 16 * table + 8;
                let relative = u32::from_be_bytes(font[position..position + 4].try_into().unwrap());
                bytes[offset + position..offset + position + 4]
                    .copy_from_slice(&(relative + offset as u32).to_be_bytes());
            }
        }
        (bytes, offsets)
    }

    #[test]
    fn selected_collection_face_keeps_valid_offsets_and_distinct_glyphs() {
        let definitions = egui::FontDefinitions::default();
        let proportional = definitions.font_data["Ubuntu-Light"].font.as_ref();
        let monospace = definitions.font_data["Hack"].font.as_ref();
        let (collection, offsets) = font_collection(&[proportional, monospace]);
        let selected = &collection[offsets[1]..];
        // GDI's zero table starts at the selected face, while its table offsets
        // still point into the original collection. This cannot be parsed alone.
        assert!(FontRef::try_from_slice(selected).is_err());

        let bytes = read_font_data(|table, buffer| match table {
            COLLECTION_TAG => copy_font_data(&collection, buffer),
            0 => copy_font_data(selected, buffer),
            _ => GDI_ERROR,
        })
        .unwrap();
        let first = FontRef::try_from_slice_and_index(&bytes, 0).unwrap();
        let selected = FontRef::try_from_slice_and_index(&bytes, 1).unwrap();
        let expected = FontRef::try_from_slice(monospace).unwrap();
        let advance = |font: &FontRef<'_>, character| {
            font.h_advance_unscaled(font.glyph_id(character)) / font.units_per_em().unwrap()
        };
        assert_ne!(advance(&first, 'i'), advance(&first, 'W'));
        assert_eq!(advance(&selected, 'i'), advance(&selected, 'W'));
        for character in ['A', 'i', 'W', 'é'] {
            assert_eq!(selected.glyph_id(character), expected.glyph_id(character));
            assert_eq!(advance(&selected, character), advance(&expected, character));
            assert!(selected.outline(selected.glyph_id(character)).is_some());
        }
    }

    #[test]
    fn standalone_font_is_read_when_collection_table_is_unavailable() {
        let definitions = egui::FontDefinitions::default();
        let source = definitions.font_data["Hack"].font.as_ref();
        let bytes = read_font_data(|table, buffer| match table {
            COLLECTION_TAG => GDI_ERROR,
            0 => copy_font_data(source, buffer),
            _ => unreachable!(),
        })
        .unwrap();
        assert_eq!(bytes, source);
        let font = FontRef::try_from_slice(&bytes).unwrap();
        assert!(font.outline(font.glyph_id('A')).is_some());
    }

    #[test]
    fn invalid_collection_sizes_do_not_allocate_or_fall_back_to_partial_data() {
        let definitions = egui::FontDefinitions::default();
        let source = definitions.font_data["Hack"].font.as_ref();
        for length in [0, MAX_FONT_BYTES + 1] {
            let result = read_font_data(|table, buffer| match table {
                COLLECTION_TAG => {
                    assert!(buffer.is_none(), "invalid size must not be allocated");
                    length
                }
                0 => copy_font_data(source, buffer),
                _ => unreachable!(),
            });
            assert!(result.is_err(), "invalid collection size {length}");
        }
        assert!(
            read_font_data(|_, buffer| {
                assert!(buffer.is_none(), "unavailable data must not be allocated");
                GDI_ERROR
            })
            .is_err()
        );
    }

    #[test]
    fn incomplete_or_failed_reads_are_rejected() {
        for returned in [0, 15, GDI_ERROR] {
            let result = read_font_data(|_, buffer| match buffer {
                None => 16,
                Some(_) => returned,
            });
            assert!(result.is_err(), "read returned {returned} of 16 bytes");
        }
    }
}
