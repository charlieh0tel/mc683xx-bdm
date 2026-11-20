# Host-pod protocol

How a host talks to the pod: over USB, or over a stream socket to
anything that answers as the pod does (m683xx-emu's virtual BDM
cable).  It is the complete specification: a stand-in written from
it alone answers as the pod does.  The pod's implementation is
`firmware/pod-proto/src/usb.rs`; where the two disagree, that is a bug
in one of them.

Protocol version: **7** (`PROTOCOL_VERSION`), raised on any change to a
request or reply.

## Transport

USB:

- VID:PID 1209:0001 (pid.codes' test pair, until the pod has its own).
- Vendor class (0xFF), interface 0, one bulk OUT and one bulk IN
  endpoint, 64-byte packets (full speed).
- Interface 1: picotool's reset interface (vendor class, subclass 0,
  protocol 1, no endpoints; pico-sdk `reset_interface.h`).  A class or
  vendor control OUT request 1 to it, wIndex its number, reboots the
  RP2040 into its USB bootloader; wValue's low 7 bits are the boot
  ROM's interface-disable mask.  `bdm bootsel` and `picotool -f` send
  it.
- One request per OUT packet, one reply per IN packet.  An OUT
  packet longer than 64 bytes is answered BadRequest.
- A request that goes 8 s without a word over the BDM link is a
  firmware fault: the pod's watchdog resets it (PLAN.md, Firmware,
  Watchdog), so it leaves the bus without a reply and enumerates
  again.  A host that waits for a reply gives up well before.  An idle
  host never trips it.

Stream (Unix or TCP socket):

- Each message, either way, is one length byte (1-64) followed by that
  many bytes: the packet USB would carry.
- A length byte of 0 or over 64 is a broken stream: the receiver
  closes the connection.
- One client at a time; a server closes any other connection at once
  while its client is connected.  A client that has closed its end is
  gone even if the server has not yet read its end of file: before
  refusing a connection, a server waits up to 100 ms for the current
  client's end of file.  Each `bdm` run is one connection, so runs
  follow each other within milliseconds.

Either way:

- Strictly one request, then its reply.  The host reads the reply
  before sending the next request.  No pipelining, no batching beyond
  what one request carries (15 raw words, 56 memory bytes).
- Multi-byte fields of this protocol are little-endian.  Memory bytes
  are the target's, in address order (big-endian values).
- A host sends Hello first and stops if the version is not its own.

## Requests

Byte 0 is the code; the body follows.  Every body length is exact, as
the table gives it; any other length, an empty packet or an unknown
code is answered BadRequest.  "FREEZE" means refused with NoFreeze
unless the target is in background mode, since a command's clocks
would otherwise be BKPT pulses that halt it.

| Code | Request | Body | Reply body (status Ok) | FREEZE |
|---|---|---|---|---|
| 0x01 | Transfer | 1-15 words, u16 each (2-30 bytes, even) | one u32 per word: the 17 bits shifted in, bit 16 the status bit, bits 31-17 zero | no |
| 0x02 | Pins | none | u8 levels, u16 VDD in mV, ATEMP field | no |
| 0x03 | ResetHalt | none | ATEMP field | no |
| 0x04 | Halt | none | ATEMP field | no |
| 0x05 | Go | none | none | yes |
| 0x06 | SetClock | u32 DSCLK Hz | u32 DSCLK Hz set | no |
| 0x07 | ReadRegister | u8 register code | u32 value | yes |
| 0x08 | WriteRegister | u8 register code, u32 value | none | yes |
| 0x09 | ReadSystemRegister | u8 select code | u32 value | yes |
| 0x0A | WriteSystemRegister | u8 select code, u32 value | none | yes |
| 0x0B | ReadMemory | u32 address, u8 size, u8 length | exactly length bytes | yes |
| 0x0C | WriteMemory | u32 address, u8 size, 1-56 data bytes (6-61 bytes in all) | none | yes |
| 0x0D | Call | u32 address | none | yes |
| 0x0E | Hello | none | u32 protocol version | no |
| 0x0F | ResetRun | none | none | no |
| 0x10 | Resync | none | ATEMP field, kind 2 | yes |
| 0x11 | Version | none | the firmware's version, ASCII: crate version and git commit (`0.1.0 1a2b3c4`, `-dirty` if built with uncommitted changes) | no |

Fields:

- Transfer word: the 16 data bits sent.  The 17th, the control bit
  (CPU32RM 7.2.7), always goes out 0.
- Register code: D0-D7 are 0-7, A0-A7 are 8-15 (RDREG/RAREG's field,
  CPU32RM 7.2.8.4).  16-255 are BadRequest.
- System register select code (CPU32RM 7.2.8.6): RPC 0, PCC 1, ATEMP 8,
  FAR 9, VBR 0xA, SR 0xB, USP 0xC, SSP 0xD, SFC 0xE, DFC 0xF, for reads
  and writes alike.  Any other byte is BadRequest.
- Size: 0 byte, 1 word, 2 long (CPU32RM 7.2.8.1).  Any other byte, 3
  included, is BadRequest.
- Memory length: 1-56 bytes, a multiple of the size; otherwise
  BadRequest.  Reads go through SFC, writes through DFC; the host sets
  them.
- Address: any 32-bit value.  The pod does not check alignment: a word
  or long access at an odd address is the target's address error,
  BusError.
- DSCLK Hz: any u32 (below).
- Pins levels: bit 0 FREEZE, bit 1 RESET, bit 2 DS, bit 3 BERR; set
  means high; bits 4-7 zero.  DSO is not reported.
- VDD: millivolts, read through the pod's divider, saturating at
  65535.  A stand-in with no supply to measure reports 5000.
- ATEMP field: u8 kind, then u32 ATEMP.  Kind 0: not in background
  mode, ATEMP 0 (Pins with FREEZE low only).  Kind 1, fresh: read first
  after entry, so the entry source.  Kind 2, stale: the pod did not see
  the entry (it started with FREEZE high), so commands may have come
  first and the value is not an entry source.  Kind 3, unread: in
  background mode, but the read after entry failed; ATEMP 0.  Pins
  carries kind 0 exactly when FREEZE is low; Halt and ResetHalt never
  carry it.

Semantics:

- Order of checks: the request is parsed (BadRequest); then, except
  for ResetHalt, ResetRun, Resync and Version, the ATEMP read below, whose failure is the reply's status (once: the
  pod keeps kind 3 and does not read again until FREEZE moves, so
  Pins, Transfer and ResetHalt still work); then the
  request's own checks (NoTarget, then NoFreeze); then the command.
- Transfer is raw: the words go out as given, with no guard and no DSI
  release, whether or not the target is in background mode.  Every
  other command is sequenced by the pod (CPU32RM 7.2.8): operands out,
  NOPs until the result, not-ready retried up to 1000 NOPs at each
  step, then NotReady.
- ReadMemory and WriteMemory: the accesses in address order, each
  `size` bytes, as READ then DUMPs or WRITE then FILLs.  On BusError
  the accesses before the failing one have happened; a write is not
  undone.
- Before carrying out any request, the pod reads ATEMP if FREEZE is
  high and FREEZE has had an edge since its last read, so ATEMP is the
  first command after every entry, the CPU's own (BGND, a breakpoint, a
  double bus fault) included.  It keeps the value until FREEZE moves,
  and reports it in Pins, Halt and ResetHalt: 0 hardware breakpoint, 1
  BGND, SSW in the high word and 0xFFFF in the low for a double bus
  fault (CPU32RM Table 7-2).
- Halt on a target already in background mode returns the kept ATEMP
  and does nothing else.  Otherwise Halt holds BKPT low until FREEZE
  asserts.  ResetHalt holds BKPT low and pulses RESET low for 10 ms,
  whatever the target's state.  Both are refused with NoTarget below
  4.5 V VDD, and NoFreeze if FREEZE does not assert within 100 ms
  (then BKPT is released).
- ResetRun drives BKPT high and DSI off, pulses RESET low for 10 ms
  and waits up to 100 ms for RESET to rise: the target comes out of
  reset running, background mode off until the next ResetHalt.
  Refused with NoTarget below 4.5 V VDD.
- Resync brings a halted CPU's serial link back into frame, as after a
  pod restart under it (lone DSCLK edges while the Pico boots): it
  clocks NOP pairs, and while the second does not read "command
  complete" (CPU32RM 7.2.8.15) adds one lone DSCLK edge, at most 17
  times; Unexpected if it never does.  Then it reads ATEMP and
  reports it stale.  The pod frames the link this way itself before
  the ATEMP read of a CPU halted before it started.  A command the CPU
  took out of frame may take the NOPs as operands.  NoTarget below
  4.5 V; NoFreeze if FREEZE is low.
- Halt works only if BDM was enabled at the target's last reset (BKPT
  low as RESET rises, MC68331UM 5.10.2.1), as ResetHalt does.  Without
  it BKPT is a breakpoint exception, vector $0C (MC68331UM 5.10.2.2.1,
  Table 5-3): the program takes it, and Halt answers NoFreeze.
- Go and Call release DSI after their last word (PLAN.md, PIO
  program); nothing is clocked after them.  After their Ok the target
  runs: Go at RPC, Call at the address with the return address stacked
  (CPU32RM 7.2.8.12, 7.2.8.13).
- SetClock sets DSCLK's rate and replies with the rate in effect: at
  most 4 MHz, and never faster than asked unless asked for less than
  the slowest rate the pod's divider gives (about 120 Hz), which it
  then sets; 0 counts as 1.  A stand-in with no clock answers with the
  asked rate held to 1 Hz-4 MHz.
- No state carries from one request to the next except the target's
  own, DSCLK's rate and the kept ATEMP.

## Replies

Byte 0 is the status.  The reply body follows only for Ok, exactly as
the request table gives it; every other status is a one-byte reply.

| Status | Name | Meaning |
|---|---|---|
| 0 | Ok | Done; the body is the result |
| 1 | BadRequest | Not a request the pod parses: unknown code, wrong length, field out of range |
| 2 | NoTarget | Target VDD below 4.5 V |
| 3 | NoFreeze | FREEZE did not assert, or is not asserted for a request that needs it |
| 4 | BusError | The access ended in a bus or address error |
| 5 | Illegal | The target refused the command |
| 6 | NotReady | Still not ready after the retries |
| 7 | Unexpected | A response the command's sequence does not allow, or the pod could not read VDD |

## Checking

`cargo test -p pod-proto` round-trips every request and reply; a
change here goes with a change to `usb.rs` and its tests, and a raised
`PROTOCOL_VERSION` if the wire changes.
