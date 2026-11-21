//! A stand-in pod on a stream socket: `mc683xx_bdm_proto::pod` over a model
//! board (`mc683xx_bdm_model::board`), answering as the pod does, for tests of
//! the host side and for trying `bdm` with no hardware.

use std::io::Read;
use std::io::Write;
use std::sync::Mutex;
use std::sync::PoisonError;

use embassy_futures::block_on;
use mc683xx_bdm_proto::pod::Board;
use mc683xx_bdm_proto::pod::Pod;
use mc683xx_bdm_proto::usb::PACKET;

use crate::transport::TransportError;
use crate::transport::read_message;
use crate::transport::write_message;

/// Answers requests on `stream` until it ends.  The pod is locked for
/// each request only, so a test can act on the board between them.
///
/// # Errors
/// The stream failed or broke the framing (docs/usb-protocol.md,
/// Transport): the caller closes it.
pub fn serve<B: Board>(mut stream: impl Read + Write, pod: &Mutex<Pod<B>>) -> Result<(), TransportError> {
    let mut request = [0; PACKET];
    let mut reply = [0; PACKET];
    while let Some(length) = read_message(&mut stream, &mut request)? {
        let mut pod = pod.lock().unwrap_or_else(PoisonError::into_inner);
        let reply_length = block_on(pod.handle(&request[..length], &mut reply));
        drop(pod);
        write_message(&mut stream, &reply[..reply_length])?;
    }
    Ok(())
}
