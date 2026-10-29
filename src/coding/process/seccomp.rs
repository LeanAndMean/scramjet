const ALLOWED: [u32; 54] = [
    0, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 21, 23, 28, 32, 33, 39, 59, 60, 61,
    62, 63, 72, 78, 79, 89, 96, 97, 99, 102, 104, 107, 108, 131, 158, 186, 202, 217, 218, 231, 247,
    257, 262, 273, 292, 302, 318, 334,
];

pub(super) fn filter() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((20 + 2 * ALLOWED.len()) * 8);
    let mut instruction = |code: u16, jt: u8, jf: u8, k: u32| {
        bytes.extend_from_slice(&code.to_ne_bytes());
        bytes.push(jt);
        bytes.push(jf);
        bytes.extend_from_slice(&k.to_ne_bytes());
    };
    instruction(0x20, 0, 0, 4);
    instruction(0x15, 1, 0, 0xc000003e);
    instruction(0x06, 0, 0, 0x80000000);
    instruction(0x20, 0, 0, 0);
    instruction(0x35, 0, 1, 0x40000000);
    instruction(0x06, 0, 0, 0x80000000);
    instruction(0x15, 0, 10, 157);
    instruction(0x20, 0, 0, 16);
    instruction(0x15, 0, 7, 1);
    instruction(0x20, 0, 0, 20);
    instruction(0x15, 0, 5, 0);
    instruction(0x20, 0, 0, 24);
    instruction(0x15, 0, 3, 9);
    instruction(0x20, 0, 0, 28);
    instruction(0x15, 0, 1, 0);
    instruction(0x06, 0, 0, 0x7fff0000);
    instruction(0x06, 0, 0, 0x00050001);
    instruction(0x20, 0, 0, 0);
    for number in ALLOWED {
        instruction(0x15, 0, 1, number);
        instruction(0x06, 0, 0, 0x7fff0000);
    }
    instruction(0x06, 0, 0, 0x00050001);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    fn action(arch: u32, syscall: u32) -> u32 {
        action_args(arch, syscall, [0, 0])
    }
    fn action_args(arch: u32, syscall: u32, args: [u64; 2]) -> u32 {
        let bytes = filter();
        let mut index = 0;
        let mut accumulator = 0;
        loop {
            let instruction = &bytes[index * 8..(index + 1) * 8];
            let code = u16::from_ne_bytes(instruction[..2].try_into().unwrap());
            let value = u32::from_ne_bytes(instruction[4..8].try_into().unwrap());
            match code {
                0x20 => {
                    accumulator = match value {
                        0 => syscall,
                        4 => arch,
                        16 => args[0] as u32,
                        20 => (args[0] >> 32) as u32,
                        24 => args[1] as u32,
                        28 => (args[1] >> 32) as u32,
                        _ => panic!(),
                    };
                    index += 1;
                }
                0x15 => {
                    index += 1 + if accumulator == value {
                        instruction[2]
                    } else {
                        instruction[3]
                    } as usize
                }
                0x35 => {
                    index += 1 + if accumulator >= value {
                        instruction[2]
                    } else {
                        instruction[3]
                    } as usize
                }
                0x06 => return value,
                _ => panic!(),
            }
        }
    }
    #[test]
    fn parent_death_binding_is_allowed_but_cannot_be_cleared_or_changed() {
        assert_eq!(action_args(0xc000003e, 157, [1, 9]), 0x7fff0000);
        for args in [
            [1, 0],
            [1, 15],
            [2, 0],
            [38, 1],
            [1 + (1 << 32), 9],
            [1, 9 + (1 << 32)],
        ] {
            assert_eq!(action_args(0xc000003e, 157, args), 0x00050001);
        }
    }
    #[test]
    fn alternate_abi_and_unselected_effect_syscalls_fail_closed() {
        for arch in [0x40000003, 0xc00000b7, 0, 0xc000003e] {
            for syscall in [0x40000000, 0x4000003b, u32::MAX] {
                assert_eq!(action(arch, syscall), 0x80000000);
            }
            if arch != 0xc000003e {
                assert_eq!(action(arch, 0), 0x80000000);
            }
        }
        for syscall in [
            22, 41, 56, 57, 58, 101, 165, 166, 175, 246, 298, 310, 311, 319, 321, 322, 323, 425,
            426, 427, 435, 29, 30, 31, 64, 65, 66, 67, 68, 69, 70, 71,
        ] {
            assert_eq!(action(0xc000003e, syscall), 0x00050001, "{syscall}");
        }
        for syscall in ALLOWED {
            assert_eq!(action(0xc000003e, syscall), 0x7fff0000);
        }
    }
}
