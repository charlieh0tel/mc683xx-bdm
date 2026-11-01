//! The pod from the host: its USB requests (`pod_proto::usb`) as calls,
//! over a [`transport::Transport`]: USB for the pod, a fake for tests;
//! and the target behind it as `mc683xx_target::Cpu32Target`.

pub mod pod;
#[cfg(test)]
mod script;
pub mod target;
pub mod transport;
