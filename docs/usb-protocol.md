# Host-pod protocol

How a host talks to the pod: over USB, or over a stream socket to
anything that answers as the pod does (m683xx-emu's virtual BDM
cable).  Source of truth: `firmware/pod-proto/src/usb.rs`; this
document follows it.

Protocol version: **1** (`PROTOCOL_VERSION`), raised on any change to a
request or reply.

## Transport

USB:

- VID:PID 1209:0001 (pid.codes' test pair, until the pod has its own).
- Vendor class (0xFF), interface 0, one bulk OUT and one bulk IN
  endpoint, 64-byte packets (full speed).
- One request per OUT packet, one reply per IN packet.

Stream (Unix or TCP socket):

- Each message, either way, is one length byte (1-64) followed by that
  many bytes: the packet USB would carry.

Either way:

- Strictly one request, then its reply.  The host reads the reply
  before sending the next request.  No pipelining, no batching beyond
  what one request carries (15 raw words, 56 memory bytes).
- Multi-byte fields of this protocol are little-endian.  Memory bytes
  are the target's, in address order (big-endian values).
- A host sends Hello first and stops if the version is not its own.

## Requests

Byte 0 is the code; the body follows.  "FREEZE" means refused with
NoFreeze unless the target is in background mode, since a command's
clocks would otherwise be BKPT pulses that halt it.

| Code | Request | Body | Reply body (status Ok) | FREEZE |
|---|---|---|---|---|
| 0x01 | Transfer | 1-15 words, u16 each | one u32 per word: the 17 bits shifted in, bit 16 the status bit | no |
| 0x02 | Pins | none | u8 levels, u16 VDD in mV | no |
| 0x03 | ResetHalt | none | u32 ATEMP | no |
| 0x04 | Halt | none | u32 ATEMP | no |
| 0x05 | Go | none | none | yes |
| 0x06 | SetClock | u32 DSCLK Hz | u32 DSCLK Hz set | no |
| 0x07 | ReadRegister | u8 register code | u32 value | yes |
| 0x08 | WriteRegister | u8 register code, u32 value | none | yes |
| 0x09 | ReadSystemRegister | u8 select code | u32 value | yes |
| 0x0A | WriteSystemRegister | u8 select code, u32 value | none | yes |
| 0x0B | ReadMemory | u32 address, u8 size, u8 length | length bytes | yes |
| 0x0C | WriteMemory | u32 address, u8 size, 1-56 data bytes | none | yes |
| 0x0D | Call | u32 address | none | yes |
| 0x0E | Hello | none | u32 protocol version | no |

Fields:

- Register code: D0-D7 are 0-7, A0-A7 are 8-15 (RDREG/RAREG's field,
  CPU32RM 7.2.8.4).
- System register select code (CPU32RM 7.2.8.6): RPC 0, PCC 1, ATEMP 8,
  FAR 9, VBR 0xA, SR 0xB, USP 0xC, SSP 0xD, SFC 0xE, DFC 0xF.  Others are
  refused.
- Size: 0 byte, 1 word, 2 long (CPU32RM 7.2.8.1); 3 is refused.
- Memory length: 1-56 bytes, a multiple of the size.  Reads go through
  SFC, writes through DFC; the host sets them.
- Pins levels: bit 0 FREEZE, bit 1 RESET, bit 2 DS; set means high.
  DSO is not reported; BERR is not sensed.

Semantics:

- Transfer is raw: the words go out as given, with no guard and no DSI
  release.  Every other command is sequenced by the pod (CPU32RM 7.2.8):
  operands out, NOPs until the result, not-ready retried up to 1000
  NOPs.
- Halt and ResetHalt read ATEMP as their first command after entry and
  return it: 0 hardware breakpoint, 1 BGND, SSW in the high word and
  0xFFFF in the low for a double bus fault (CPU32RM Table 7-2).  Both
  are refused with NoTarget below 4.5 V VDD, and NoFreeze if FREEZE
  does not assert within 100 ms.
- Halt works only if BDM was enabled at the target's last reset (BKPT
  low as RESET rises, MC68331UM 5.10.2.1), as ResetHalt does.  Without
  it BKPT is a breakpoint exception, vector $0C (MC68331UM 5.10.2.2.1,
  Table 5-3): the program takes it, and Halt answers NoFreeze.
- Go and Call release DSI after their last word (PLAN.md, PIO
  program); nothing is clocked after them.
- SetClock never sets a rate faster than asked, nor above 4 MHz.

## Replies

Byte 0 is the status; the reply body follows only for Ok.

| Status | Name | Meaning |
|---|---|---|
| 0 | Ok | Done; the body is the result |
| 1 | BadRequest | Not a request the pod parses: unknown code, wrong length, field out of range |
| 2 | NoTarget | Target VDD below 4.5 V |
| 3 | NoFreeze | FREEZE did not assert, or is not asserted for a request that needs it |
| 4 | BusError | The access ended in a bus or address error |
| 5 | Illegal | The target refused the command |
| 6 | NotReady | Still not ready after the retries |
| 7 | Unexpected | A response the command's sequence does not allow |

## Checking

`cargo test -p pod-proto` round-trips every request and reply; a
change here goes with a change to `usb.rs` and its tests, and a raised
`PROTOCOL_VERSION` if the wire changes.
