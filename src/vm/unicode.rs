use super::*;
use unicode_normalization::UnicodeNormalization;

impl Vm {
    pub(super) fn unicode_transform(
        &mut self,
        selector: u32,
        args: &[u32],
    ) -> Result<u32, VmError> {
        let address = args.first().copied().unwrap_or(0);
        let capacity = args.get(1).copied().unwrap_or(0);
        let count = args.get(2).copied().unwrap_or(0).min(capacity);
        let mut input = String::new();
        for i in 0..count {
            input.push(char::from_u32(self.memory.read32(address + i * 4)?).unwrap_or('\u{fffd}'));
        }
        let mut output = Vec::new();
        match selector {
            0x0123 => output.extend(input.nfd().map(|c| c as u32)),
            0x0124 => output.extend(input.nfc().map(|c| c as u32)),
            _ => {
                for (index, character) in input.chars().enumerate() {
                    let mapped = match selector {
                        0x0120 => unicode_case_mapping::to_lowercase(character).to_vec(),
                        0x0121 => unicode_case_mapping::to_uppercase(character).to_vec(),
                        _ if index == 0 => unicode_case_mapping::to_titlecase(character).to_vec(),
                        _ if args.get(3).copied().unwrap_or(0) != 0 => {
                            unicode_case_mapping::to_lowercase(character).to_vec()
                        }
                        _ => vec![character as u32],
                    };
                    if mapped.iter().all(|c| *c == 0) {
                        output.push(character as u32);
                    } else {
                        output.extend(mapped.into_iter().filter(|c| *c != 0));
                    }
                }
            }
        }
        for (i, &value) in output.iter().take(capacity as usize).enumerate() {
            self.memory.write32(address + i as u32 * 4, value)?;
        }
        Ok(output.len() as u32)
    }
}
