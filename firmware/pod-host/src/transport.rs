//! How a request packet reaches the pod and its reply comes back.

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use nusb::DeviceInfo;
use nusb::Endpoint;
use nusb::MaybeFuture;
use nusb::descriptors::TransferType;
use nusb::transfer::Buffer;
use nusb::transfer::Bulk;
use nusb::transfer::ControlOut;
use nusb::transfer::ControlType;
use nusb::transfer::Direction;
use nusb::transfer::In;
use nusb::transfer::Out;
use nusb::transfer::Recipient;
use pod_proto::usb::PACKET;
use pod_proto::usb::PID;
use pod_proto::usb::VID;

/// How long a reply may take: a reset into background mode is the
/// longest, about 110 ms (`pod-fw`'s RESET pulse and FREEZE timeout).
const TIMEOUT: Duration = Duration::from_secs(1);

/// The pod's request interface (`pod-fw`'s first vendor function).
const INTERFACE: u8 = 0;

/// picotool's reset interface (`pod-fw`'s `reset`): vendor class,
/// subclass 0, protocol 1, and its request into the bootloader.
const RESET_CLASS: (u8, u8, u8) = (0xFF, 0x00, 0x01);
const RESET_REQUEST_BOOTSEL: u8 = 0x01;

/// The boot ROM's USB device, RP2040 and RP2350.
const BOOTLOADER_VID: u16 = 0x2E8A;
const BOOTLOADER_PIDS: [u16; 2] = [0x0003, 0x000F];
/// How long the bootloader may take to appear, and how often to look.
const BOOTLOADER_WAIT: Duration = Duration::from_secs(5);
const BOOTLOADER_POLL: Duration = Duration::from_millis(100);

/// Carries one request packet out and its reply packet back.
pub trait Transport {
    /// Sends `request` and fills `reply`, returning the reply's length.
    ///
    /// # Errors
    /// The request or reply did not get through.
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError>;
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError> {
        (**self).exchange(request, reply)
    }
}

/// Why a packet did not get through.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// No pod is plugged in.
    #[error("no pod found (USB {VID:04x}:{PID:04x})")]
    NotFound,
    /// Opening or claiming the pod failed.
    #[error("cannot open the pod")]
    Open(#[from] nusb::Error),
    /// The pod's interface has no bulk endpoint in this direction.
    #[error("the pod has no bulk {0:?} endpoint")]
    NoEndpoint(Direction),
    /// A transfer failed or timed out.
    #[error("USB transfer failed")]
    Transfer(#[from] nusb::transfer::TransferError),
    /// The socket failed.
    #[error("socket failed")]
    Socket(#[from] std::io::Error),
    /// The pod's firmware has no reset interface: one older than the
    /// knock, which needs the BOOTSEL button.
    #[error("the pod has no reset interface; hold BOOTSEL and replug it")]
    NoResetInterface,
    /// The bootloader did not appear after the request.
    #[error("no bootloader appeared within {BOOTLOADER_WAIT:?}")]
    NoBootloader,
    /// A stream message's length byte was 0 or more than a packet.
    #[error("bad message length {0}")]
    MessageLength(usize),
}

/// The first pod found on USB.
fn find_pod() -> Result<DeviceInfo, TransportError> {
    nusb::list_devices()
        .wait()?
        .find(|device| device.vendor_id() == VID && device.product_id() == PID)
        .ok_or(TransportError::NotFound)
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
        let info = find_pod()?;
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

/// Reboots the first pod found into the RP2040's USB bootloader, as
/// `picotool reboot -u` does, and waits for the bootloader to appear.
///
/// # Errors
/// No pod, no reset interface, or no bootloader afterwards.
pub fn reboot_to_bootloader() -> Result<(), TransportError> {
    let device = find_pod()?.open().wait()?;
    let number = device
        .active_configuration()
        .map_err(|_| TransportError::NoResetInterface)?
        .interface_alt_settings()
        .find(|alt| (alt.class(), alt.subclass(), alt.protocol()) == RESET_CLASS)
        .map(|alt| alt.interface_number())
        .ok_or(TransportError::NoResetInterface)?;
    let interface = device.claim_interface(number).wait()?;
    let request = ControlOut {
        control_type: ControlType::Class,
        recipient: Recipient::Interface,
        request: RESET_REQUEST_BOOTSEL,
        value: 0,
        index: u16::from(number),
        data: &[],
    };
    // The pod leaves the bus before it can answer, so the transfer's
    // own status says nothing: the bootloader appearing is the answer.
    let sent = interface.control_out(request, TIMEOUT).wait();
    let started = std::time::Instant::now();
    while started.elapsed() < BOOTLOADER_WAIT {
        let found = nusb::list_devices().wait()?.any(|device| {
            device.vendor_id() == BOOTLOADER_VID && BOOTLOADER_PIDS.contains(&device.product_id())
        });
        if found {
            return Ok(());
        }
        std::thread::sleep(BOOTLOADER_POLL);
    }
    sent?;
    Err(TransportError::NoBootloader)
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

/// A pod (or the emulator standing in for one) on a stream socket: each
/// message, either way, is one length byte (1 to [`PACKET`]) and that
/// many bytes (docs/usb-protocol.md, Transport).
#[derive(Debug)]
pub struct StreamTransport<S> {
    stream: S,
}

impl StreamTransport<UnixStream> {
    /// Connects to a Unix socket.
    ///
    /// # Errors
    /// The socket cannot be connected.
    pub fn unix(path: &str) -> Result<Self, TransportError> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        Ok(Self::new(stream))
    }
}

impl StreamTransport<TcpStream> {
    /// Connects to a TCP socket, `host:port`.
    ///
    /// # Errors
    /// The socket cannot be connected.
    pub fn tcp(address: &str) -> Result<Self, TransportError> {
        let stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_nodelay(true)?;
        Ok(Self::new(stream))
    }
}

impl<S> StreamTransport<S> {
    /// The pod at the end of `stream`.
    pub const fn new(stream: S) -> Self {
        Self { stream }
    }
}

impl<S: Read + Write> Transport for StreamTransport<S> {
    fn exchange(&mut self, request: &[u8], reply: &mut [u8; PACKET]) -> Result<usize, TransportError> {
        write_message(&mut self.stream, request)?;
        read_message(&mut self.stream, reply)?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into())
    }
}

/// Writes one stream message: its length byte, then `packet`.
///
/// # Errors
/// A packet of 0 or more than [`PACKET`] bytes, or the stream failed.
pub fn write_message(stream: &mut impl Write, packet: &[u8]) -> Result<(), TransportError> {
    let length = u8::try_from(packet.len())
        .ok()
        .filter(|n| (1..=PACKET).contains(&usize::from(*n)))
        .ok_or(TransportError::MessageLength(packet.len()))?;
    stream.write_all(&[length])?;
    stream.write_all(packet)?;
    stream.flush()?;
    Ok(())
}

/// Reads one stream message into `packet`, returning its length, or
/// `None` if the stream ended before a message began.
///
/// # Errors
/// A length byte of 0 or more than [`PACKET`], or the stream failed.
pub fn read_message(
    stream: &mut impl Read,
    packet: &mut [u8; PACKET],
) -> Result<Option<usize>, TransportError> {
    let mut length = [0];
    if stream.read(&mut length)? == 0 {
        return Ok(None);
    }
    let length = usize::from(length[0]);
    if !(1..=PACKET).contains(&length) {
        return Err(TransportError::MessageLength(length));
    }
    stream.read_exact(&mut packet[..length])?;
    Ok(Some(length))
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::thread;

    use pod_proto::usb::PACKET;

    use super::StreamTransport;
    use super::Transport;
    use super::TransportError;

    #[test]
    fn stream_messages_are_length_prefixed() {
        let (host, mut pod) = UnixStream::pair().unwrap();
        let server = thread::spawn(move || {
            let mut length = [0];
            pod.read_exact(&mut length).unwrap();
            let mut request = vec![0; usize::from(length[0])];
            pod.read_exact(&mut request).unwrap();
            pod.write_all(&[5, 0, 1, 0, 0, 0]).unwrap();
            request
        });
        let mut transport = StreamTransport::new(host);
        let mut reply = [0; PACKET];
        assert_eq!(transport.exchange(&[0x0E], &mut reply).unwrap(), 5);
        assert_eq!(&reply[..5], &[0, 1, 0, 0, 0]);
        assert_eq!(server.join().unwrap(), [0x0E]);
    }

    #[test]
    fn bad_lengths_are_refused() {
        let (host, mut pod) = UnixStream::pair().unwrap();
        let mut transport = StreamTransport::new(host);
        let mut reply = [0; PACKET];
        assert!(matches!(transport.exchange(&[], &mut reply), Err(TransportError::MessageLength(0))));
        pod.write_all(&[0]).unwrap();
        assert!(matches!(transport.exchange(&[0x02], &mut reply), Err(TransportError::MessageLength(0))));
    }
}
