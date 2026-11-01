//! What the pod and its host share: the CPU32 BDM serial protocol
//! (`bdm`) and the command sequences built on it (`session`), which run
//! in the firmware and are tested here on the host.

#![cfg_attr(not(test), no_std)]

pub mod bdm;
pub mod session;
