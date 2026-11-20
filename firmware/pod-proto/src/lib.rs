//! What the pod and its host share: the CPU32 BDM serial protocol
//! (`bdm`), the command sequences built on it (`session`), which run in
//! the firmware and are tested here on the host, the DSCLK divider
//! (`dsclk`), when the pod reads ATEMP (`entry`), the USB messages
//! between host and pod (`usb`), the pod's answer to each of them over
//! a board (`pod`), and when the pod feeds its watchdog (`watchdog`).

#![cfg_attr(not(test), no_std)]

pub mod bdm;
pub mod dsclk;
pub mod entry;
pub mod pod;
pub mod session;
pub mod usb;
pub mod watchdog;
