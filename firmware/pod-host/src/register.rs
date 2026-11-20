//! Registers by name, as `bdm` and the Python bindings take them, read
//! and written through any [`Cpu32Target`].

use mc683xx_target::Cpu32Target;
use mc683xx_target::register::Register;
use mc683xx_target::register::SystemRegister;

/// A register by name: a data or address register, or a system
/// register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Named {
    /// D0-D7, A0-A7.
    Data(Register),
    /// RPC, PCC, ATEMP, FAR, VBR, SR, USP, SSP, SFC, DFC.
    System(SystemRegister),
}

/// Text that names no register.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("no register {0}")]
pub struct UnknownRegister(pub String);

/// System registers in the order a register listing shows them; ATEMP
/// is left out, as reading it after entry gives only a scratch value.
pub const SYSTEM_REGISTERS: [SystemRegister; 9] = [
    SystemRegister::Rpc,
    SystemRegister::Pcc,
    SystemRegister::Sr,
    SystemRegister::Usp,
    SystemRegister::Ssp,
    SystemRegister::Vbr,
    SystemRegister::Sfc,
    SystemRegister::Dfc,
    SystemRegister::Far,
];

impl Named {
    /// The register `name` names, in any case: `d0`-`a7`, `sp` for A7,
    /// or a system register.
    ///
    /// # Errors
    /// `name` names no register.
    pub fn parse(name: &str) -> Result<Self, UnknownRegister> {
        if name.eq_ignore_ascii_case("sp") {
            return Ok(Self::Data(Register::A7));
        }
        name.parse()
            .map(Self::Data)
            .or_else(|_| name.parse().map(Self::System))
            .map_err(|_| UnknownRegister(name.to_owned()))
    }

    /// Its value in `target`.
    ///
    /// # Errors
    /// As the target's.
    pub fn read<T: Cpu32Target>(self, target: &mut T) -> Result<u32, T::Error> {
        match self {
            Self::Data(register) => target.read_register(register),
            Self::System(register) => target.read_system_register(register),
        }
    }

    /// Sets it in `target` to `value`.
    ///
    /// # Errors
    /// As the target's.
    pub fn write<T: Cpu32Target>(self, target: &mut T, value: u32) -> Result<(), T::Error> {
        match self {
            Self::Data(register) => target.write_register(register, value),
            Self::System(register) => target.write_system_register(register, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use mc683xx_target::register::Register;
    use mc683xx_target::register::SystemRegister;

    use super::Named;
    use super::UnknownRegister;

    #[test]
    fn register_names() {
        assert_eq!(Named::parse("d0"), Ok(Named::Data(Register::D0)));
        assert_eq!(Named::parse("A7"), Ok(Named::Data(Register::A7)));
        assert_eq!(Named::parse("SP"), Ok(Named::Data(Register::A7)));
        assert_eq!(Named::parse("sfc"), Ok(Named::System(SystemRegister::Sfc)));
        assert_eq!(Named::parse("d8"), Err(UnknownRegister("d8".to_owned())));
        assert!(Named::parse("x1").is_err());
        assert!(Named::parse("").is_err());
        for register in Register::ALL {
            assert_eq!(Named::parse(&register.to_string()), Ok(Named::Data(register)));
        }
        for register in SystemRegister::ALL {
            assert_eq!(Named::parse(&register.to_string()), Ok(Named::System(register)));
        }
    }
}
