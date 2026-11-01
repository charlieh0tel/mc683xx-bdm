//! A CPU32 (683xx) under debug control: the BDM pod drives silicon
//! through it, and the emulator implements it too, so the same harness
//! runs against either (PLAN.md, Goals).
//!
//! Register numbering and select codes are CPU32RM's (7.2.8).

#![no_std]

/// A data or address register, numbered as RDREG/RAREG's 4-bit field
/// numbers it: D0-D7 are 0-7, A0-A7 are 8-15 (CPU32RM 7.2.8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Register {
    /// Data register 0.
    D0,
    /// Data register 1.
    D1,
    /// Data register 2.
    D2,
    /// Data register 3.
    D3,
    /// Data register 4.
    D4,
    /// Data register 5.
    D5,
    /// Data register 6.
    D6,
    /// Data register 7.
    D7,
    /// Address register 0.
    A0,
    /// Address register 1.
    A1,
    /// Address register 2.
    A2,
    /// Address register 3.
    A3,
    /// Address register 4.
    A4,
    /// Address register 5.
    A5,
    /// Address register 6.
    A6,
    /// Address register 7, the active stack pointer.
    A7,
}

impl Register {
    /// Every register, in field order: `Register::ALL[n].code() == n`.
    pub const ALL: [Self; 16] = [
        Self::D0,
        Self::D1,
        Self::D2,
        Self::D3,
        Self::D4,
        Self::D5,
        Self::D6,
        Self::D7,
        Self::A0,
        Self::A1,
        Self::A2,
        Self::A3,
        Self::A4,
        Self::A5,
        Self::A6,
        Self::A7,
    ];

    /// The A/D and register fields together, 0-15.
    #[must_use]
    pub const fn code(self) -> u16 {
        self as u16
    }
}

/// A system register, with its RSREG/WSREG select code (CPU32RM
/// 7.2.8.6).  RPC, PCC, ATEMP and FAR exist only in background mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum SystemRegister {
    /// Return program counter: where GO resumes.
    Rpc = 0x0,
    /// Current instruction program counter.
    Pcc = 0x1,
    /// Temporary register A: the BDM entry source, read first after
    /// entry because later commands overwrite it (CPU32RM 7.2.3).
    Atemp = 0x8,
    /// Fault address register.
    Far = 0x9,
    /// Vector base register.
    Vbr = 0xA,
    /// Status register.
    Sr = 0xB,
    /// User stack pointer.
    Usp = 0xC,
    /// Supervisor stack pointer.
    Ssp = 0xD,
    /// Source function code register: the space READ and DUMP use.
    Sfc = 0xE,
    /// Destination function code register: the space WRITE and FILL use.
    Dfc = 0xF,
}

impl SystemRegister {
    /// The 4-bit select code.
    #[must_use]
    pub const fn code(self) -> u16 {
        self as u16
    }
}

/// An operand size, with its 2-bit size field (CPU32RM 7.2.8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Size {
    /// 8 bits.
    Byte = 0b00,
    /// 16 bits.
    Word = 0b01,
    /// 32 bits.
    Long = 0b10,
}

impl Size {
    /// The 2-bit size field.
    #[must_use]
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// Bytes in one operand.
    #[must_use]
    pub const fn bytes(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Word => 2,
            Self::Long => 4,
        }
    }
}

/// A 32-bit target address.  The 68331 drives 24 address lines; BDM
/// takes all 32 bits (CPU32RM 7.2.8.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address(pub u32);

/// What a debugger needs from a halted CPU32.  Memory goes through the
/// function code registers (SFC for reads, DFC for writes), so set them
/// first to reach a space other than the one they hold.
pub trait Cpu32Target {
    /// What went wrong: the link, a bus error, or the target's own.
    type Error;

    /// Resets the target and stops it in background mode before its
    /// first instruction.
    ///
    /// # Errors
    /// The target did not enter background mode.
    fn reset_halted(&mut self) -> Result<(), Self::Error>;

    /// Stops a running target in background mode.
    ///
    /// # Errors
    /// The target did not enter background mode.
    fn halt(&mut self) -> Result<(), Self::Error>;

    /// Resumes execution at RPC.
    ///
    /// # Errors
    /// The command did not complete.
    fn go(&mut self) -> Result<(), Self::Error>;

    /// Reads a data or address register.
    ///
    /// # Errors
    /// The read did not complete.
    fn read_register(&mut self, register: Register) -> Result<u32, Self::Error>;

    /// Writes a data or address register, all 32 bits.
    ///
    /// # Errors
    /// The write did not complete.
    fn write_register(&mut self, register: Register, value: u32) -> Result<(), Self::Error>;

    /// Reads a system register.
    ///
    /// # Errors
    /// The read did not complete.
    fn read_system_register(&mut self, register: SystemRegister) -> Result<u32, Self::Error>;

    /// Writes a system register.
    ///
    /// # Errors
    /// The write did not complete.
    fn write_system_register(&mut self, register: SystemRegister, value: u32) -> Result<(), Self::Error>;

    /// Reads `data.len()` bytes from `address` in `size` accesses,
    /// big-endian.  `data.len()` is a multiple of `size.bytes()`.
    ///
    /// # Errors
    /// A bus error, or a length that is not a multiple of the size.
    fn read_memory(&mut self, address: Address, size: Size, data: &mut [u8]) -> Result<(), Self::Error>;

    /// Writes `data` to `address` in `size` accesses, big-endian.
    /// `data.len()` is a multiple of `size.bytes()`.
    ///
    /// # Errors
    /// A bus error, or a length that is not a multiple of the size.
    fn write_memory(&mut self, address: Address, size: Size, data: &[u8]) -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::Register;
    use super::Size;
    use super::SystemRegister;

    #[test]
    fn register_codes_follow_field_order() {
        for (n, register) in Register::ALL.iter().enumerate() {
            assert_eq!(usize::from(register.code()), n);
        }
        assert_eq!(Register::D7.code(), 7);
        assert_eq!(Register::A0.code(), 8);
        assert_eq!(Register::A7.code(), 15);
    }

    #[test]
    fn system_register_codes_are_cpu32rm_select_codes() {
        assert_eq!(SystemRegister::Rpc.code(), 0b0000);
        assert_eq!(SystemRegister::Pcc.code(), 0b0001);
        assert_eq!(SystemRegister::Atemp.code(), 0b1000);
        assert_eq!(SystemRegister::Far.code(), 0b1001);
        assert_eq!(SystemRegister::Vbr.code(), 0b1010);
        assert_eq!(SystemRegister::Sr.code(), 0b1011);
        assert_eq!(SystemRegister::Usp.code(), 0b1100);
        assert_eq!(SystemRegister::Ssp.code(), 0b1101);
        assert_eq!(SystemRegister::Sfc.code(), 0b1110);
        assert_eq!(SystemRegister::Dfc.code(), 0b1111);
    }

    #[test]
    fn size_fields_and_widths() {
        assert_eq!(Size::Byte.code(), 0b00);
        assert_eq!(Size::Word.code(), 0b01);
        assert_eq!(Size::Long.code(), 0b10);
        assert_eq!(Size::Byte.bytes(), 1);
        assert_eq!(Size::Word.bytes(), 2);
        assert_eq!(Size::Long.bytes(), 4);
    }
}
