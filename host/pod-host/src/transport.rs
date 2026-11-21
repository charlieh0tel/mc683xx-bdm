//! How a request packet reaches the pod and its reply comes back.

use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::str::FromStr;
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
    /// No new bootloader appeared after the request.
    #[error("no new bootloader appeared within {BOOTLOADER_WAIT:?}")]
    NoBootloader,
    /// A stream message's length byte was 0 or more than a packet.
    #[error("bad message length {0}")]
    MessageLength(usize),
    /// More than one pod matches, and none was named: their serial
    /// numbers.
    #[error("{} pods connected ({}): name one, usb:SERIAL", .0.len(), serial_list(.0))]
    Ambiguous(Vec<Option<String>>),
    /// No pod has the serial number asked for.
    #[error("no pod with serial number {serial} (connected: {})", serial_list(.connected))]
    NoSuchSerial {
        /// The serial number asked for.
        serial: String,
        /// The connected pods' serial numbers.
        connected: Vec<Option<String>>,
    },
    /// Text that names no pod address.
    #[error("the pod is usb, usb:SERIAL, unix:PATH or tcp:HOST:PORT, not {0}")]
    BadAddress(String),
}

/// Shown for a pod with no serial number: firmware older than it.
pub const NO_SERIAL: &str = "-";

/// Serial numbers for a message, [`NO_SERIAL`] for a pod without one.
fn serial_list(serials: &[Option<String>]) -> String {
    if serials.is_empty() {
        return "none".to_owned();
    }
    serials.iter().map(|serial| serial.as_deref().unwrap_or(NO_SERIAL)).collect::<Vec<_>>().join(", ")
}

/// Where the pod is, as `bdm --pod` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PodAddress {
    /// On USB: the one with this serial number, or with `None` the only
    /// one connected.
    Usb(Option<String>),
    /// A stand-in on a Unix socket at this path.
    Unix(String),
    /// A stand-in on TCP, `host:port`.
    Tcp(String),
}

impl FromStr for PodAddress {
    type Err = TransportError;

    /// `usb`, `usb:SERIAL`, `unix:PATH` or `tcp:HOST:PORT`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once(':') {
            None if text == "usb" => Ok(Self::Usb(None)),
            Some(("usb", serial)) if !serial.is_empty() => Ok(Self::Usb(Some(serial.to_owned()))),
            Some(("unix", path)) if !path.is_empty() => Ok(Self::Unix(path.to_owned())),
            Some(("tcp", address)) if !address.is_empty() => Ok(Self::Tcp(address.to_owned())),
            _ => Err(TransportError::BadAddress(text.to_owned())),
        }
    }
}

impl PodAddress {
    /// Connects to the pod.
    ///
    /// # Errors
    /// No such pod, more than one where none was named, or it cannot
    /// be opened or connected.
    pub fn connect(&self) -> Result<Box<dyn Transport + Send>, TransportError> {
        Ok(match self {
            Self::Usb(serial) => Box::new(UsbTransport::open(serial.as_deref())?),
            Self::Unix(path) => Box::new(StreamTransport::unix(path)?),
            Self::Tcp(address) => Box::new(StreamTransport::tcp(address)?),
        })
    }
}

/// A pod on USB, as listed without opening it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbPod {
    /// Its serial number (docs/usb-protocol.md); `None` from firmware
    /// older than it.
    pub serial: Option<String>,
    /// Where it is plugged in: bus, then the hub port chain, as Linux
    /// names it in sysfs (`1-2.3`).
    pub location: String,
}

/// A location from a bus and a hub port chain: `1-2.3`.
fn location(bus: &str, ports: &[u8]) -> String {
    let ports: Vec<String> = ports.iter().map(u8::to_string).collect();
    format!("{bus}-{}", ports.join("."))
}

/// Every pod on USB, by its VID:PID.
fn pod_devices() -> Result<Vec<DeviceInfo>, TransportError> {
    Ok(nusb::list_devices()
        .wait()?
        .filter(|device| device.vendor_id() == VID && device.product_id() == PID)
        .collect())
}

/// Every pod on USB, without opening any.
///
/// # Errors
/// USB devices cannot be listed.
pub fn list_pods() -> Result<Vec<UsbPod>, TransportError> {
    Ok(pod_devices()?
        .iter()
        .map(|device| UsbPod {
            serial: device.serial_number().map(str::to_owned),
            location: location(device.bus_id(), device.port_chain()),
        })
        .collect())
}

/// Which of the pods with these serial numbers to use: the one whose
/// serial number is `wanted`, ignoring case, or with `None` the only
/// one.
///
/// # Errors
/// No pod, no pod with that serial number, or more than one match.
fn select(serials: &[Option<&str>], wanted: Option<&str>) -> Result<usize, TransportError> {
    let owned = |serials: &[Option<&str>]| serials.iter().map(|serial| serial.map(str::to_owned)).collect();
    let matching: Vec<usize> = (0..serials.len())
        .filter(|&n| {
            wanted.is_none_or(|wanted| serials[n].is_some_and(|serial| serial.eq_ignore_ascii_case(wanted)))
        })
        .collect();
    match (matching.as_slice(), wanted) {
        ([n], _) => Ok(*n),
        ([], None) => Err(TransportError::NotFound),
        ([], Some(wanted)) => {
            Err(TransportError::NoSuchSerial { serial: wanted.to_owned(), connected: owned(serials) })
        }
        _ => {
            let matched: Vec<Option<&str>> = matching.iter().map(|&n| serials[n]).collect();
            Err(TransportError::Ambiguous(owned(&matched)))
        }
    }
}

/// The pod on USB with serial number `serial`, or with `None` the only
/// one (`select`).
fn find_pod(serial: Option<&str>) -> Result<DeviceInfo, TransportError> {
    let devices = pod_devices()?;
    let serials: Vec<Option<&str>> = devices.iter().map(DeviceInfo::serial_number).collect();
    let chosen = select(&serials, serial)?;
    devices.into_iter().nth(chosen).ok_or(TransportError::NotFound)
}

/// The pod on USB.
#[derive(Debug)]
pub struct UsbTransport {
    to_pod: Endpoint<Bulk, Out>,
    from_pod: Endpoint<Bulk, In>,
}

impl UsbTransport {
    /// Opens the pod with serial number `serial`, or with `None` the
    /// only one connected.
    ///
    /// # Errors
    /// No such pod, more than one where none was named, or it cannot be
    /// opened or claimed.
    pub fn open(serial: Option<&str>) -> Result<Self, TransportError> {
        let info = find_pod(serial)?;
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

/// A USB device by where it is: one that enumerates again (a pod
/// rebooted into its boot ROM) gets a new address.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BusAddress {
    /// The bus, as the OS names it.
    bus: String,
    /// The device's address on it.
    address: u8,
}

/// Every boot ROM device (RP2040, RP2350) on USB.
fn bootloaders() -> Result<Vec<BusAddress>, TransportError> {
    Ok(nusb::list_devices()
        .wait()?
        .filter(|device| {
            device.vendor_id() == BOOTLOADER_VID && BOOTLOADER_PIDS.contains(&device.product_id())
        })
        .map(|device| BusAddress { bus: device.bus_id().to_owned(), address: device.device_address() })
        .collect())
}

/// Whether `now` lists a boot ROM device `before` did not: the pod,
/// rebooted.  One already in BOOTSEL before the request is another
/// board.
fn appeared(before: &[BusAddress], now: &[BusAddress]) -> bool {
    now.iter().any(|device| !before.contains(device))
}

/// Reboots the pod with serial number `serial`, or with `None` the only
/// one connected, into the RP2040's USB bootloader, as `picotool reboot
/// -u` does, and waits for a new boot ROM device to appear.
///
/// # Errors
/// No such pod, more than one where none was named, no reset interface,
/// or no new bootloader afterwards.
pub fn reboot_to_bootloader(serial: Option<&str>) -> Result<(), TransportError> {
    let device = find_pod(serial)?.open().wait()?;
    let before = bootloaders()?;
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
    // own status says nothing: a new bootloader appearing is the answer.
    let sent = interface.control_out(request, TIMEOUT).wait();
    let started = std::time::Instant::now();
    while started.elapsed() < BOOTLOADER_WAIT {
        if appeared(&before, &bootloaders()?) {
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
/// `None` if the stream ended before a message began.  A read a signal
/// interrupts is made again, as `read_exact` makes its own.
///
/// # Errors
/// A length byte of 0 or more than [`PACKET`], or the stream failed.
pub fn read_message(
    stream: &mut impl Read,
    packet: &mut [u8; PACKET],
) -> Result<Option<usize>, TransportError> {
    let mut length = [0];
    let read = loop {
        match stream.read(&mut length) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            read => break read?,
        }
    };
    if read == 0 {
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

    use super::BusAddress;
    use super::PodAddress;
    use super::StreamTransport;
    use super::Transport;
    use super::TransportError;
    use super::appeared;
    use super::location;
    use super::read_message;
    use super::select;

    const A: &str = "E6613852833B2A2F";
    const B: &str = "E66138528361572C";

    /// A boot ROM device at `address` on bus 1.
    fn device(address: u8) -> BusAddress {
        BusAddress { bus: "1".to_owned(), address }
    }

    #[test]
    fn only_a_new_bootloader_is_the_pods() {
        let other = device(5);
        assert!(!appeared(&[], &[]));
        assert!(!appeared(std::slice::from_ref(&other), std::slice::from_ref(&other)), "already in BOOTSEL");
        assert!(appeared(std::slice::from_ref(&other), &[other.clone(), device(9)]));
        assert!(appeared(&[], &[device(9)]));
        assert!(appeared(&[device(9)], &[BusAddress { bus: "2".to_owned(), address: 9 }]), "another bus");
        assert!(!appeared(&[other, device(9)], &[device(9)]), "one left, none came");
    }

    #[test]
    fn one_pod_is_chosen_unnamed() {
        assert_eq!(select(&[Some(A)], None).unwrap(), 0);
        assert_eq!(select(&[None], None).unwrap(), 0);
    }

    /// A stream whose first read a signal interrupts.
    struct Interrupted<'a> {
        first: bool,
        rest: &'a [u8],
    }

    impl Read for Interrupted<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if std::mem::take(&mut self.first) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            self.rest.read(buffer)
        }
    }

    #[test]
    fn a_read_a_signal_interrupts_is_made_again() {
        let mut stream = Interrupted { first: true, rest: &[2, 0xAB, 0xCD] };
        let mut packet = [0; PACKET];
        assert_eq!(read_message(&mut stream, &mut packet).unwrap(), Some(2));
        assert_eq!(packet[..2], [0xAB, 0xCD]);
    }

    #[test]
    fn no_pod_is_not_found() {
        assert!(matches!(select(&[], None), Err(TransportError::NotFound)));
    }

    #[test]
    fn several_pods_unnamed_are_refused_with_their_serials() {
        let Err(TransportError::Ambiguous(serials)) = select(&[Some(A), None, Some(B)], None) else {
            panic!("not refused")
        };
        assert_eq!(serials, [Some(A.to_owned()), None, Some(B.to_owned())]);
        let message = TransportError::Ambiguous(serials).to_string();
        assert_eq!(message, format!("3 pods connected ({A}, -, {B}): name one, usb:SERIAL"));
    }

    #[test]
    fn a_named_pod_is_chosen_among_several_ignoring_case() {
        assert_eq!(select(&[Some(A), None, Some(B)], Some(B)).unwrap(), 2);
        assert_eq!(select(&[Some(A), Some(B)], Some(&A.to_lowercase())).unwrap(), 0);
        assert_eq!(select(&[Some(A)], Some(A)).unwrap(), 0);
    }

    #[test]
    fn a_missing_serial_lists_the_connected_pods() {
        let error = select(&[Some(A), None], Some("0123")).unwrap_err();
        assert_eq!(error.to_string(), format!("no pod with serial number 0123 (connected: {A}, -)"));
        let error = select(&[], Some(A)).unwrap_err();
        assert_eq!(error.to_string(), format!("no pod with serial number {A} (connected: none)"));
    }

    #[test]
    fn a_duplicated_serial_is_refused() {
        let Err(TransportError::Ambiguous(serials)) = select(&[Some(A), Some(B), Some(A)], Some(A)) else {
            panic!("not refused")
        };
        assert_eq!(serials, [Some(A.to_owned()), Some(A.to_owned())]);
    }

    #[test]
    fn pod_addresses() {
        assert_eq!("usb".parse::<PodAddress>().unwrap(), PodAddress::Usb(None));
        assert_eq!(format!("usb:{A}").parse::<PodAddress>().unwrap(), PodAddress::Usb(Some(A.to_owned())));
        assert_eq!(
            "unix:/tmp/pod.sock".parse::<PodAddress>().unwrap(),
            PodAddress::Unix("/tmp/pod.sock".to_owned())
        );
        assert_eq!(
            "tcp:localhost:2000".parse::<PodAddress>().unwrap(),
            PodAddress::Tcp("localhost:2000".to_owned())
        );
        for bad in ["", "usb:", "unix:", "tcp:", "USB", "serial:1", "usbx"] {
            assert!(matches!(bad.parse::<PodAddress>(), Err(TransportError::BadAddress(_))), "{bad}");
        }
    }

    #[test]
    fn locations_are_bus_then_ports() {
        assert_eq!(location("1", &[2, 3]), "1-2.3");
        assert_eq!(location("3", &[1]), "3-1");
    }

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
