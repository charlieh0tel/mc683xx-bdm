//! What the pod and its host share: the CPU32 BDM serial protocol
//! (`bdm`), the command sequences built on it (`session`), which run in
//! the firmware and are tested here on the host, the DSCLK divider
//! (`dsclk`) and the USB messages between host and pod (`usb`).

#![cfg_attr(not(test), no_std)]

pub mod bdm;
pub mod dsclk;
pub mod session;
pub mod usb;
