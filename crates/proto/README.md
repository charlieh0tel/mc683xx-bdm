# mc683xx-bdm-proto

What the mc683xx BDM pod's firmware and its host share.  The pod is a
USB background debug mode (BDM) pod for Motorola/Freescale CPU32
(683xx) microcontrollers, built on a Raspberry Pi Pico.

- `bdm`: the CPU32 BDM serial protocol (CPU32RM section 7).
- `session`: the command sequences built on it, which run in the
  firmware.
- `usb`: the messages between host and pod, and `PROTOCOL_VERSION`.
- `pod`: the pod's answer to each message, over a board.
- `dsclk`, `entry`, `watchdog`: the DSCLK divider, when the pod reads
  ATEMP, when it feeds its watchdog.

`no_std`, without allocation.  The host library and the `bdm` command
line are `mc683xx-bdm`.

The pod, its firmware, the protocol's specification
(`docs/usb-protocol.md`) and the carrier board:
https://github.com/charlieh0tel/mc683xx-bdm.

License: GPL-3.0-only.
