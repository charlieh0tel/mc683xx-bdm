//! A memory access as Python gives it: a size in bytes, a function code
//! as a number, and a single value as big-endian bytes.

use mc683xx_target::function_code::FunctionCode;
use mc683xx_target::size::Size;
use pod_proto::bdm::byte_count;

use crate::error::Error;

/// Bytes in the widest access, a long.
const LONG_BYTES: usize = 4;

/// The access size of `bytes` bytes: 1, 2 or 4.
pub(crate) const fn size(bytes: u32) -> Result<Size, Error> {
    match bytes {
        1 => Ok(Size::Byte),
        2 => Ok(Size::Word),
        4 => Ok(Size::Long),
        _ => Err(Error::Size(bytes)),
    }
}

/// The function code `bits` gives: 0 to 7.
pub(crate) fn function_code(bits: u32) -> Result<FunctionCode, Error> {
    pod_host::pod::function_code(bits).ok_or(Error::FunctionCode(bits))
}

/// `value` as one big-endian access of `size`: the bytes, of which the
/// first `size.bytes()` are the access's.
pub(crate) fn encode(size: Size, value: u32) -> Result<[u8; LONG_BYTES], Error> {
    if value & !size.mask() != 0 {
        return Err(Error::Width { value, bytes: size.bytes() });
    }
    let bytes = value.to_be_bytes();
    let mut access = [0; LONG_BYTES];
    let length = byte_count(size);
    access[..length].copy_from_slice(&bytes[LONG_BYTES - length..]);
    Ok(access)
}

/// The value of one big-endian access, `bytes`.
pub(crate) fn decode(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |value, byte| value << u8::BITS | u32::from(*byte))
}

#[cfg(test)]
mod tests {
    use mc683xx_target::function_code::FunctionCode;
    use mc683xx_target::size::Size;

    use super::decode;
    use super::encode;
    use super::function_code;
    use super::size;
    use crate::error::Error;

    #[test]
    fn sizes_are_bytes() {
        assert_eq!(size(1).unwrap(), Size::Byte);
        assert_eq!(size(2).unwrap(), Size::Word);
        assert_eq!(size(4).unwrap(), Size::Long);
        for bytes in [0, 3, 8] {
            assert!(matches!(size(bytes), Err(Error::Size(n)) if n == bytes));
        }
    }

    #[test]
    fn function_codes_fit_three_bits() {
        assert_eq!(function_code(6).unwrap(), FunctionCode::SUPERVISOR_PROGRAM);
        assert!(matches!(function_code(8), Err(Error::FunctionCode(8))));
    }

    #[test]
    fn values_are_big_endian() {
        assert_eq!(encode(Size::Word, 0x1234).unwrap()[..2], [0x12, 0x34]);
        assert_eq!(encode(Size::Long, 0xDEAD_BEEF).unwrap(), [0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(encode(Size::Byte, 0xFF).unwrap()[..1], [0xFF]);
        assert!(matches!(encode(Size::Byte, 0x100), Err(Error::Width { value: 0x100, bytes: 1 })));
        assert_eq!(decode(&[0x12, 0x34]), 0x1234);
        assert_eq!(decode(&[0xDE, 0xAD, 0xBE, 0xEF]), 0xDEAD_BEEF);
    }
}
