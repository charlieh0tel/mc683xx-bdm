//! How a request packet reaches the pod and its reply comes back.

use std::time::Duration;

use nusb::Endpoint;
use nusb::MaybeFuture;
use nusb::descriptors::TransferType;
use nusb::transfer::Buffer;
use nusb::transfer::Bulk;
use nusb::transfer::Direction;
use nusb::transfer::In;
use nusb::transfer::Out;
use pod_proto::usb::PACKET;
use pod_proto::usb::PID;
use pod_proto::usb::VID;

/// How long a reply may take: a reset into background mode is the
/// longest, about 110 ms (`pod-fw`'s RESET pulse and FREEZE timeout).
const TIMEOUT: Duration = Duration::from_secs(1);

/// The pod's one interface (`pod-fw`'s vendor function).
const INTERFACE: u8 = 0;

/// Carries one request packet out and its reply packet back.
pub trait Transport {
    /// Sends `request` and fills `reply`, returning the reply's length.
    ///
    /// # Errors
    /// The request or reply did not get through.
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError>;
}

/// Why a packet did not get through.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// No pod is plugged in.
    #[error("no pod found (USB {VID:04x}:{PID:04x})")]
    NotFound,
    /// Opening or claiming the pod failed.
    #[error("cannot open the pod: {0}")]
    Open(#[from] nusb::Error),
    /// The pod's interface has no bulk endpoint in this direction.
    #[error("the pod has no bulk {0:?} endpoint")]
    NoEndpoint(Direction),
    /// A transfer failed or timed out.
    #[error("USB transfer failed: {0}")]
    Transfer(#[from] nusb::transfer::TransferError),
}

/// The pod on USB.
#[derive(Debug)]
pub struct UsbTransport {
    to_pod: Endpoint<Bulk, Out>,
    from_pod: Endpoint<Bulk, In>,
}

impl UsbTransport {
    /// Opens the first pod found.
    ///
    /// # Errors
    /// No pod, or it cannot be opened or claimed.
    pub fn open() -> Result<Self, TransportError> {
        let info = nusb::list_devices()
            .wait()?
            .find(|device| device.vendor_id() == VID && device.product_id() == PID)
            .ok_or(TransportError::NotFound)?;
        let interface = info.open().wait()?.claim_interface(INTERFACE).wait()?;
        let bulk = |direction: Direction| {
            interface
                .descriptor()
                .and_then(|descriptor| {
                    descriptor.endpoints().find(|endpoint| {
                        endpoint.transfer_type() == TransferType::Bulk && endpoint.direction() == direction
                    })
                })
                .map(|endpoint| endpoint.address())
                .ok_or(TransportError::NoEndpoint(direction))
        };
        let to_pod = interface.endpoint::<Bulk, Out>(bulk(Direction::Out)?)?;
        let from_pod = interface.endpoint::<Bulk, In>(bulk(Direction::In)?)?;
        Ok(Self { to_pod, from_pod })
    }
}

impl Transport for UsbTransport {
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError> {
        let mut out = Buffer::new(request.len());
        out.extend_from_slice(request);
        self.to_pod.transfer_blocking(out, TIMEOUT).status?;
        let done = self.from_pod.transfer_blocking(Buffer::new(PACKET), TIMEOUT);
        done.status?;
        let length = done.actual_len.min(PACKET);
        reply[..length].copy_from_slice(&done.buffer[..length]);
        Ok(length)
    }
}
