//! The pod from the host: its USB requests (`pod_proto::usb`) as calls,
//! over a [`transport::Transport`]: USB for the pod, a fake for tests.

pub mod pod;
pub mod transport;
